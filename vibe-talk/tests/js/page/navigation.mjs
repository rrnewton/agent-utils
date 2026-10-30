// The app page's tests: where the reader is: the inbox, channels and their names, views and their memory, Back.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/navigation.mjs.

import assert from "node:assert/strict";

import { ALIAS_NOTICE, CHANNEL, CSS_CODE, DISCORD_POLL_MS, HTML, HTML_CODE, MESSAGE_CACHE_KEY,
  OTHER_CHANNEL, SCRIPT, SCRIPT_CODE, SPACE_A, SPACE_B, TEAM_A, TEAM_B, TOKEN_STORAGE_KEY,
  UI_STATE_KEY, assertCurrent, assistantSays, atBottomOf, backFacts, backlog, beforeOf,
  brokenScript, browsingPage, cardsUp, changeChannelTo, channelBoxCode, channelReads, channelRow,
  chooseService, conversation, cssBlock, delayedReply, deliver, doneButton, errorResponse,
  focusName, focused, forwardTimeline, freshness, gate, holdCountsRead, json, jumpSaysArrived,
  longMessage, mainOnlyThreadPage, message, newPage, nodeIn, nodeWithinFixture, offline,
  openBrowser, openReplyOn, openerOf, optionText, outgoingRows, pagedThreads, pickThread,
  pickerChoices, press, reReadChannel, readKinds, reloadPage, reloadWith, reopenedChannel,
  replayedFrames, replyBoxAt, replyButton, replyInStream, row, rowMoreButton, rowMoreMenu,
  rowShowing, rowState, rowWithId, savedScopes, savedUiState, scopeKey, scopedData, scopedPage,
  scriptFolds, serveScoped, serviceOf, servicesPage, settingsChannel, settingsFoldState,
  settingsOptions, settingsSection, showDiscord, shownIds, sideIds, sideRow, signIn, sourceConstant,
  sseDelete, sseMessage, sseUpdate, startTalking, tallChannel, test, threadBadge, threadButton,
  threadData, threadOptions, threadPage, timelineAnswer, turnTodoOn, twoColumns, twoServices,
  viewOf, wideCard, withLiveChannel, withReplies } from "./fixture.mjs";

const archiveButton = (li) =>
  li.descendants().find((node) => node.className === "archive-button");

test("A MESSAGE SOMEBODY HAS ANSWERED IS RECORDED AS ANSWERED, BUT ONLY DONE DRAWS IT GREY", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, conversation());

  // Both axes named, because they are independent: being ANSWERED is derived from what happens to
  // be loaded, being ARCHIVED is the reader's own declaration, and neither implies the other.
  assert.deepStrictEqual(rowState(page, 0), { replied: "true", archived: "false", reading: "false", pending: "false", bookmark: null, isReply: "false" });
  // The ANSWER is not itself answered. Discord marks only the answering message, so a naive
  // implementation that set the flag on whichever row carried a pointer would dim this one.
  assert.deepStrictEqual(rowState(page, 1), { replied: "false", archived: "false", reading: "false", pending: "false", bookmark: null, isReply: "true" });
  assert.deepStrictEqual(rowState(page, 2), { replied: "false", archived: "false", reading: "false", pending: "false", bookmark: null, isReply: "false" });

  // `#256 reply-marks-read`: recorded, NOT drawn. Grey means read (Done); a separate "answered" grey left a
  // message looking read while its Done button said unread, which is the mismatch the owner reported.
  assert.ok(!CSS_CODE.includes('li.discord-message[data-replied="true"]'), "being answered draws its own grey again");
});

test("the replied state is re-derived over the WHOLE list, not decided when a row is built", async () => {
  // The answer to a message can arrive long after it. A row's state is a fact about the SET.
  const page = newPage();
  // `withLiveChannel` signs in and switches to the channel itself, so nothing may do either first.
  const stream = await withLiveChannel(page, [
    message({ id: "1000000000000000001", content: "open question" }),
  ]);
  assert.equal(rowState(page, 0).replied, "false", "it started out answered");

  // A live arrival that answers it.
  await deliver(
    page,
    stream,
    sseMessage(
      message({
        id: "1000000000000000002",
        content: "answering it now",
        reply_to: "1000000000000000001",
      })
    )
  );

  assert.equal(
    rowState(page, 0).replied,
    "true",
    "an arriving answer did not dim the message it answers"
  );
});

// `#50 todo-view`. THE DEFAULT STATE OF THE CHANNEL VIEW, and the half that was missing.
//
// The owner's report: archiving appeared to do nothing. It was doing something — the dismissal was
// recorded on the server and the To do filter honoured it — but the channel view said NOTHING about
// it, so a swipe on the default view produced no visible change at all. These pin the pair: greyed
// here, hidden only behind the filter, and a way back on the row itself.

test("AN ARCHIVED MESSAGE IS GREYED WHERE IT STANDS, NOT REMOVED FROM THE CHANNEL VIEW", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "first" }),
    message({ id: "1000000000000000002", content: "second" }),
  ]);

  assert.equal(rowState(page, 0).archived, "false", "a fresh row is already archived");
  await doneButton(row(page, 0)).click();
  await page.settle();

  assert.equal(
    page.el("discord-log").children.length,
    2,
    "archiving REMOVED the row; the channel view greys, it does not hide — that is the filter's job"
  );
  assert.equal(rowState(page, 0).archived, "true", "the archived row was not greyed");
  assert.equal(rowState(page, 1).archived, "false", "archiving one row greyed another");

  // And it is DRAWN, not merely recorded in an attribute.
  const greyed = cssBlock('#discord-log li.discord-message[data-archived="true"]');
  assert.match(greyed, /opacity/, "an archived row is not dimmed at all");
  // Desaturated as well as dimmer. A `me` or `coder` row keeps its speaker tint when it is merely
  // ANSWERED, so opacity alone cannot tell the reader which of the two states they are looking at.
  assert.match(
    greyed,
    /grayscale/,
    "an archived row keeps its speaker colour, so it reads as merely answered"
  );
});

test("the archived row offers the way back, and taking it restores the row in place", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001", content: "first" })]);

  await doneButton(row(page, 0)).click();
  await page.settle();
  assert.equal(
    doneButton(row(page, 0)).text(),
    "Unarchive",
    "an archived row still offers to archive itself again"
  );

  await doneButton(row(page, 0)).click();
  await page.settle();
  assert.equal(rowState(page, 0).archived, "false", "unarchiving did not bring the row back");
  assert.equal(doneButton(row(page, 0)).text(), "Done", "the control did not go back to Done");
  assert.deepEqual(
    page.restoreCalls,
    [{ messages: ["1000000000000000001"] }],
    "the way back did not post the restoration to the server"
  );
});

test("the row's own control is reachable in the CHANNEL view, not only behind the filter", async () => {
  // It used to be hidden outside the To do filter, from a time when an archived row was never on
  // screen here. Now that it is, hiding its only keyboard-reachable control would leave the swipe
  // as the sole way in — and a gesture must never be the only way to reach an act.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);

  assert.equal(
    doneButton(row(page, 0)).hidden,
    false,
    "the channel view hides the only control a keyboard can reach"
  );
});

test("the archive survives the poll that rebuilds the list", async () => {
  // The set is the SERVER's answer, re-read with every page. A browser-local set would be lost the
  // moment the list was replaced, and the row would silently un-grey itself under the reader.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await doneButton(row(page, 0)).click();
  await page.settle();

  await reReadChannel(page);

  assert.equal(
    rowState(page, 0).archived,
    "true",
    "a re-read of the channel forgot what had been archived"
  );
});

test("THE SWIPE DRIVES THE SAME DISMISSAL THE DONE BUTTON DOES, NOT A SECOND ONE", () => {
  // `#50 todo-view` deferred the gesture layer on one condition: the acts must exist first, each
  // reachable by a control a keyboard can also get to, so a gesture is a SECOND way in and never
  // the only one. That condition is what makes this safe — a swipe that recorded something only
  // this browser knew, with no undo, would be the worst control on the page.
  //
  // So there is exactly one notion of "dealt with", it is the server's, and the swipe posts to it.
  assert.match(
    SCRIPT_CODE,
    /function swipeable\([\s\S]*?toggleArchived\(/,
    "the swipe does not go through the same act the row's button uses"
  );
  // ...and that shared act is the SERVER's, both ways. The swipe archives an ordinary row and puts
  // an archived one back, so the gesture is its own undo on the row in front of the reader.
  assert.match(
    SCRIPT_CODE,
    /async function toggleArchived\([\s\S]*?restoreMessages\([\s\S]*?dismissMessages\(/,
    "the shared act does not reach both the dismissal and the restoration"
  );
  // And no second, browser-local one is left behind. `#84 reply-aware-dismissal` arrived with an archive kept
  // in localStorage and a filter of its own; two controls that both mean "show me what is left" is
  // worse than either alone, and the server's state is the one with an undo behind it.
  for (const gone of ["ARCHIVED_KEY", "setArchived", "inboxOnly", "renderInboxToggle"]) {
    assert.ok(!SCRIPT_CODE.includes(gone), gone + " survived the consolidation onto #50's state");
  }
  assert.ok(!HTML.includes('id="inbox-only"'), "a second filter is still in the markup");
  // TOUCH AND PEN ONLY. A horizontal mouse drag across a message is how a person selects text, and
  // this list exists so a specific real message can be quoted and checked.
  assert.match(
    SCRIPT_CODE,
    /pointerType === "mouse"/,
    "a mouse drag across a message would now dismiss it instead of selecting text"
  );
});

test("REPLYING FROM THE PAGE DIMS WHAT YOU ANSWERED, WITHOUT WAITING FOR A POLL", async () => {
  // The whole loop: the reply this page posts carries `message_reference`, so the message it
  // answers becomes an answered one immediately rather than at the next forty-five-second re-read.
  const page = newPage();
  await signIn(page);
  await openReplyOn(page, conversation(), 2);
  assert.equal(rowState(page, 2).replied, "false", "it was already dimmed before the reply");

  page.el("reply-text").value = "answering the open one";
  await page.el("reply-send").click();
  await page.settle();

  const posted = page.repliesPosted.at(-1);
  assert.equal(
    posted.body.reply_to,
    "1000000000000000003",
    "the reply did not reference the message"
  );
  assert.equal(
    rowState(page, 2).replied,
    "true",
    "the message just answered is still shown as open"
  );
});

// --- what to call a channel -----------------------------------------------------------------------
//
// `#39 channel-alias`. The owner gives a channel a name of his own, in this app, and this app then
// uses it wherever it showed the configured label — including in what the voice agent is handed,
// so the name he SAYS is the name the model was given.
//
// The browser half is what is tested here. The three claims:
//
//   * The alias wins wherever the label showed, and clearing it puts the label back — checked at
//     every place on this page that renders a channel's name, because a fix applied to the picker
//     and not to the head of the channel is how those two come to disagree.
//   * The page shows the SERVER's statement that Discord was not told, quoted, never its own.
//   * Renaming a channel is not a change of channel: the view keeps reading what it was reading.

/** The channel the fixture serves, wearing a name of the owner's. */
const ALIASED = { ...CHANNEL, alias: "the build channel" };

test("THE CHANNEL BOX IS SMALL WHEN IT IS DOING NOTHING", async () => {
  // The owner's complaint about the first version: a rename box, an id box, a name box and a
  // checkbox all standing open at once, which on a phone is most of a screen of text entry for two
  // acts performed rarely. At rest the box is a picker, one line of fact, and the verbs.
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();

  assert.equal(page.el("rename-fields").hidden, true, "the rename field is open before it is asked for");
  assert.equal(page.el("add-channel-fields").hidden, true, "the add form is open before it is asked for");
  // ...and the verbs ARE there, or the box would be a dead end rather than a compact one.
  for (const id of ["rename-channel", "open-add-channel"]) {
    assert.equal(page.el(id).hidden, false, `${id} is not offered`);
  }
  // One line saying what this channel is, including where it came from — which is what explains
  // the absence of a Remove button rather than leaving it unexplained.
  assert.match(page.el("channel-facts").text(), /lead team/, "the box does not say what channel this is");
  assert.match(page.el("channel-facts").text(), /configuration file/, "the box does not say where it came from");
});

test("...Rename opens the field, and Cancel closes it without keeping what was typed", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();

  await page.el("rename-channel").click();
  assert.equal(page.el("rename-fields").hidden, false, "Rename opened nothing");
  assert.equal(
    page.el("rename-channel").getAttribute("aria-expanded"),
    "true",
    "the disclosure does not say it is open, so a screen reader cannot tell"
  );

  page.el("channel-alias").value = "typed but not saved";
  await page.el("cancel-rename").click();
  assert.equal(page.el("rename-fields").hidden, true, "Cancel left the field open");
  assert.deepEqual(page.aliasCalls, [], "Cancel saved the name anyway");

  // Re-opening shows what the SERVER says, not what was abandoned.
  await page.el("rename-channel").click();
  assert.notEqual(
    page.el("channel-alias").value,
    "typed but not saved",
    "an abandoned rename came back the next time the panel opened"
  );
});

test("...only one panel is open at a time, and changing channel closes both", async () => {
  // Both panels are text entry, and two open at once is the full-height form this replaced.
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();

  await page.el("rename-channel").click();
  await page.el("open-add-channel").click();
  assert.equal(page.el("rename-fields").hidden, true, "opening Add left Rename open underneath it");
  assert.equal(page.el("add-channel-fields").hidden, false);

  // A half-typed name belongs to the channel that was chosen when it was typed. Carrying it to a
  // different one is worse than losing it.
  page.el("new-channel-id").value = "1110000000000000007";
  page.el("new-channel-label").value = "another";
  await page.el("add-channel").click();
  await page.settle();
  assert.equal(page.el("add-channel-fields").hidden, true, "the form stayed open after it succeeded");

  await page.el("rename-channel").click();
  page.el("settings-channel").value = "1110000000000000007";
  await page.el("settings-channel").dispatch("change");
  assert.equal(
    page.el("rename-fields").hidden,
    true,
    "a rename panel opened against one channel survived a change to another"
  );
});

test("A CHANNEL CAN BE ADDED FROM INSIDE THE APP, AND APPEARS IN THE PICKER", async () => {
  // The whole point: a second channel without editing a file or redeploying. What is asserted is
  // not that a request went out but that the channel is now SWITCHABLE — the picker is the feature.
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  assert.equal(
    page.el("discord-channel").children.length,
    1,
    "the fixture already has more than one channel, so adding one proves nothing"
  );

  await page.el("open-add-channel").click();
  assert.equal(page.el("add-channel-fields").hidden, false, "Add a channel did not open the form");
  page.el("new-channel-id").value = "1110000000000000002";
  page.el("new-channel-label").value = "second team";
  await page.el("add-channel").click();
  await page.settle();

  assert.deepEqual(
    page.addChannelCalls,
    [{ id: "1110000000000000002", label: "second team", writable: false }],
    "the id and name did not reach the server, or writable defaulted ON"
  );
  const options = page.el("discord-channel").children.map((o) => o.value);
  assert.deepEqual(
    options,
    ["1110000000000000001", "1110000000000000002"],
    "the new channel is not in the picker, so it cannot be switched to"
  );
  assert.match(page.el("add-channel-state").text(), /Added/, "nothing said it worked");
  assert.equal(page.el("new-channel-id").value, "", "the form kept the id it had just used");
});

test("MANAGED REGISTRATION HIDES THE CHECKBOX AND ACCEPTS BRIDGE WRITE POLICY", async () => {
  const page = newPage();
  page.channelRegistrationSupported = true;
  page.registeredChannelWritable = true;
  await signIn(page);
  await page.el("open-settings").click();
  await page.el("open-add-channel").click();

  assert.match(page.el("new-channel-id-label").text(), /Channel link or provider reference/);
  assert.match(page.el("new-channel-help").text(), /from Discord\. The channel is checked/);
  assert.doesNotMatch(page.el("new-channel-help").text(), /bridge/i);
  assert.equal(
    page.el("new-channel-writable-row").hidden,
    true,
    "managed registration exposed a posting capability it does not accept"
  );

  const source = "https://chat.example/space/thread";
  page.el("new-channel-id").value = source;
  page.el("new-channel-label").value = "release thread";
  await page.el("add-channel").click();
  await page.settle();

  assert.deepEqual(page.addChannelCalls, [{ source, label: "release thread" }]);
  assert.equal(
    page.channels.find((channel) => channel.id === page.registeredChannelId).writable,
    true,
    "the managed response lost the bridge-declared write policy"
  );
  assert.ok(
    page.el("discord-channel").children.some((option) => option.value === page.registeredChannelId),
    "the picker used the source instead of the bridge-issued id"
  );
});

test("...and letting the bridge POST there is a deliberate act, off by default", async () => {
  // The one control on this screen that widens what the bridge can SAY rather than what it reads.
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  await page.el("open-add-channel").click();
  page.el("new-channel-id").value = "1110000000000000003";
  page.el("new-channel-label").value = "writable one";
  page.el("new-channel-writable").checked = true;
  await page.el("add-channel").click();
  await page.settle();

  assert.equal(
    page.addChannelCalls[0].writable,
    true,
    "the reader asked for a writable channel and got a read-only one"
  );
});

test("...and a channel the bot cannot read is refused IN THE SERVER'S OWN WORDS", async () => {
  // The server knows whether this was a typo, a channel the bot was never added to, or a token
  // that has stopped working. A sentence written in the page could only be vaguer than that one.
  const page = newPage();
  page.addChannelError = "the bot is not in that channel; add it there, not only to the server";
  await signIn(page);
  await page.el("open-settings").click();
  await page.el("open-add-channel").click();
  page.el("new-channel-id").value = "1110000000000000009";
  page.el("new-channel-label").value = "not mine";
  await page.el("add-channel").click();
  await page.settle();

  assert.match(
    page.el("add-channel-state").text(),
    /not in that channel/,
    "the reason was replaced with a vaguer one of the page's own"
  );
  assert.equal(
    page.el("discord-channel").children.length,
    1,
    "a channel that was refused was added to the picker anyway"
  );
});

test("REMOVING A CHANNEL ADDED HERE FORGETS IT", async () => {
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();

  await page.el("open-add-channel").click();
  page.el("new-channel-id").value = "1110000000000000004";
  page.el("new-channel-label").value = "temporary";
  await page.el("add-channel").click();
  await page.settle();
  assert.equal(page.el("remove-channel").hidden, false, "an added channel cannot be removed");

  await page.el("remove-channel").click();
  await page.settle();
  assert.deepEqual(page.removeChannelCalls, ["1110000000000000004"]);
  assert.deepEqual(
    page.el("discord-channel").children.map((o) => o.value),
    ["1110000000000000001"],
    "the removed channel is still in the picker"
  );
  assert.match(page.el("remove-channel-state").text(), /no longer in the picker/);
  assert.equal(
    page.el("hidden-channels").hidden,
    true,
    "a forgotten added channel was offered back as a hidden configured one"
  );
});

test("REMOVE IS OFFERED FOR A CHANNEL FROM THE FILE, WHICH IS HIDDEN AND CAN BE SHOWN AGAIN", async () => {
  // `#199 removable-config-channels`. The owner could not take the deployment's one configured
  // channel off his list: the button was not offered, because the server refused. The server now
  // hides such a channel durably, so the button is offered and what it did is said afterwards.
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  assert.equal(
    page.el("remove-channel").hidden,
    false,
    "a channel from the configuration file still cannot be removed from the list"
  );
  assert.equal(page.el("hidden-channels").hidden, true, "an empty hidden list was shown");

  await page.el("remove-channel").click();
  await page.settle();
  assert.deepEqual(page.removeChannelCalls, [CHANNEL.id]);
  assert.deepEqual(
    page.el("discord-channel").children.map((o) => o.value),
    [],
    "the hidden channel is still in the picker"
  );
  assert.match(page.el("remove-channel-state").text(), /configuration file/);
  assert.match(page.el("remove-channel-state").text(), /shown again/);
  // THE EMPTY STATE: the list is empty, and the box says what to do rather than that the server
  // has no channels, which is not true.
  assert.match(page.el("channel-facts").text(), /channel list is empty/);
  assert.doesNotMatch(page.el("channel-facts").text(), /no channels configured/);
  assert.equal(page.el("open-add-channel").hidden, false, "the empty list offers no way to add one");
  assert.equal(page.el("hidden-channels").hidden, false, "the hidden channel is not offered back");
  const rows = page.el("hidden-channel-list").children;
  assert.equal(rows.length, 1);
  assert.match(rows[0].children[0].text(), new RegExp(CHANNEL.label));
  const showAgain = rows[0].children[1];
  assert.equal(showAgain.text(), "Show again");

  await showAgain.click();
  await page.settle();
  assert.deepEqual(page.showChannelCalls, [CHANNEL.id]);
  assert.deepEqual(
    page.el("discord-channel").children.map((o) => o.value),
    [CHANNEL.id],
    "Show again did not put the channel back in the picker"
  );
  assert.equal(page.el("settings-channel").value, CHANNEL.id);
  assert.equal(page.el("hidden-channels").hidden, true, "a shown channel is still listed as hidden");
  assert.match(page.el("remove-channel-state").text(), /Shown again/);
});

test("the hidden channels client-config reports are offered back under the owner's own names", async () => {
  // Hidden on another device, or before this page loaded: the server's list is the truth, and a
  // channel he renamed is offered back under his name for it.
  const page = newPage();
  page.hiddenChannels = [
    { id: "1110000000000000007", label: "home base", alias: "the base", writable: true },
  ];
  await signIn(page);
  await page.el("open-settings").click();
  assert.equal(page.el("hidden-channels").hidden, false);
  const rows = page.el("hidden-channel-list").children;
  assert.equal(rows.length, 1);
  assert.equal(rows[0].children[0].text(), "the base");
  assert.equal(
    page.el("discord-channel").children.map((o) => o.value).includes("1110000000000000007"),
    false,
    "a hidden channel was put in the picker"
  );
});

// --- `#401 hidden-channels-fold` ------------------------------------------------------------------
//
// The owner, 2026-10-21: "That little section that shows old hidden channels. I think it should stay
// at the bottom of that pane and be collapsed by default. When I'm switching channels and seeing the
// interface update it seems invasive to have this bit in the middle that shows the old removed
// channels." tests/channel_box_browser.py measures, in Chromium, that opening it moves nothing above it.

/** Three configured channels taken off the list, as client-config reports them. */
const HIDDEN_THREE = [
  { id: "1110000000000000071", label: "old builds", alias: null, writable: true },
  { id: "1110000000000000072", label: "retired alerts", alias: null, writable: true },
  { id: "1110000000000000073", label: "launch room", alias: null, writable: true },
];

/**
 * What is wrong with the hidden channels' place against `#401`, as sentences; empty when nothing is. A
 * function of the markup, so each negative control can hand it the list somewhere else.
 */
function hiddenFoldProblems(html) {
  const found = [];
  const code = html.replace(/<!--[\s\S]*?-->/g, " ");
  const section = settingsSection("Channels", code);
  if (channelBoxCode(code).includes('id="hidden-channel-list"')) found.push("the hidden channels are inside the channel box");
  const fold = new RegExp('<section class="settings-group" id="hidden-channels" hidden>\\s*<div class="group-head">\\s*' +
    '<h3\\b[^>]*><button class="fold-toggle" id="fold-hidden-channels" type="button" aria-expanded="false" ' +
    'aria-controls="fold-hidden-channels-body">Hidden channels</button></h3>\\s*</div>\\s*' +
    '<div class="fold-body" id="fold-hidden-channels-body" hidden="until-found">([\\s\\S]*?)</div>\\s*</section>').exec(section);
  if (!fold) {
    found.push("the hidden channels are not a fold of their own, hidden while there are none");
    return found;
  }
  if (!fold[1].includes('id="hidden-channel-list"')) found.push("the list is not inside the fold");
  // The foot of the section: after the fold, only the end of the section's body.
  if (!/^\s*<\/div>\s*$/.test(section.slice(fold.index + fold[0].length))) found.push("the fold is not the last thing in the section");
  return found;
}

test("`#401 hidden-channels-fold`: the hidden channels are a fold of their own at the foot of the Channels section", () => {
  assert.deepStrictEqual(hiddenFoldProblems(HTML), []);
  const group = /<section class="settings-group" id="hidden-channels" hidden>[\s\S]*?<\/ul>\s*<\/div>\s*<\/section>\s*/.exec(HTML);
  assert.ok(group, "the negative controls cannot find the fold");
  // The negative control: the list where it was, in the middle of the box under its buttons.
  const middle = HTML.replace(group[0], "").replace('<p id="remove-channel-state" class="hint" aria-live="polite"></p>',
    '<p id="remove-channel-state" class="hint" aria-live="polite"></p><div id="hidden-channels" hidden>' +
    '<p class="hint">Hidden channels from this server\'s configuration:</p>' +
    '<ul id="hidden-channel-list" class="channel-directory" aria-label="Hidden channels"></ul></div>');
  assert.deepStrictEqual(hiddenFoldProblems(middle),
    ["the hidden channels are inside the channel box", "the hidden channels are not a fold of their own, hidden while there are none"]);
  // ...and the fold at the head of the section, above the box, where a picker change would move it.
  const above = HTML.replace(group[0], "").replace('<section class="settings-group" id="channel-settings"',
    `${group[0]}<section class="settings-group" id="channel-settings"`);
  assert.deepStrictEqual(hiddenFoldProblems(above), ["the fold is not the last thing in the section"]);
});

test("`#401 hidden-channels-fold`: folded on every load, its heading counting what is in it, and gone while nothing is", async () => {
  const page = newPage();
  page.hiddenChannels = HIDDEN_THREE.map((channel) => ({ ...channel }));
  await signIn(page);
  await page.el("open-settings").click();
  await page.el("fold-settings-channels").click();
  assert.equal(page.el("hidden-channels").hidden, false, "the hidden channels are not offered");
  assert.equal(page.el("fold-hidden-channels").textContent, "Hidden channels (3)", "the heading does not count them");
  assert.equal(settingsFoldState(page, "hidden-channels"), false, "the hidden channels are open as Settings opens");
  // Drawn inside the folded body, so the browser's find still reaches a name.
  const rows = page.el("hidden-channel-list").children;
  assert.deepStrictEqual(rows.map((row) => row.children[0].textContent), ["old builds", "retired alerts", "launch room"]);
  assert.ok(rows.every((row) => nodeIn(page.el("fold-hidden-channels-body"), row)), "a row is outside the fold");
  // A fold like the rest: its heading opens it, and Expand all counts it.
  await page.el("fold-hidden-channels").click();
  assert.equal(settingsFoldState(page, "hidden-channels"), true);
  await page.el("fold-settings-all").click();
  await page.el("fold-settings-all").click();
  assert.equal(settingsFoldState(page, "hidden-channels"), false, "Fold all left the hidden channels open");
  // A reload folds it again.
  const again = reloadWith(new Map(page.storage), []);
  again.hiddenChannels = HIDDEN_THREE.map((channel) => ({ ...channel }));
  await signIn(again);
  await again.el("open-settings").click();
  assert.equal(settingsFoldState(again, "hidden-channels"), false, "the fold was remembered across a reload");
  // None hidden: the group is gone, and Expand all does not count a fold nobody can see.
  const none = newPage();
  none.agentSettingsUrl = null;
  await signIn(none);
  await none.el("open-settings").click();
  assert.equal(none.el("hidden-channels").hidden, true, "an empty fold is offered");
  assert.equal(none.el("agent-settings").hidden, true);
  assert.equal(none.el("message-cache-group").hidden, true);
  for (const slug of scriptFolds(SCRIPT_CODE).filter((one) => !["provider", "message-cache", "hidden-channels"].includes(one))) {
    if (!foldOpenOn(none, slug)) await none.el(`fold-${slug}`).click();
  }
  assert.equal(none.el("fold-settings-all").textContent, "Fold all", "the empty fold is counted by Expand all");
  // The negative control: a heading that does not count.
  const mute = newPage(new Map(), brokenScript('  el("fold-hidden-channels").textContent = `Hidden channels (${rows.length})`;\n', ""));
  mute.hiddenChannels = HIDDEN_THREE.map((channel) => ({ ...channel }));
  await signIn(mute);
  await mute.el("open-settings").click();
  assert.equal(mute.el("fold-hidden-channels").textContent, "Hidden channels",
    "the negative control counted anyway, so the check above proves nothing");
});

/** Whether the fold `slug` is open on `page`, by its button. */
const foldOpenOn = (page, slug) => page.el(`fold-${slug}`).getAttribute("aria-expanded") === "true";

/**
 * Settings with two channels and hidden ones, the fold left `open`, then the box's picker moved twice:
 * the fold's state after each move.
 */
async function hiddenFoldAcrossPicker(open, script = SCRIPT) {
  const page = newPage(new Map(), script);
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  page.hiddenChannels = HIDDEN_THREE.map((channel) => ({ ...channel }));
  await signIn(page);
  await page.el("open-settings").click();
  await page.el("fold-settings-channels").click();
  if (open) await page.el("fold-hidden-channels").click();
  const states = [];
  for (const id of [OTHER_CHANNEL.id, CHANNEL.id]) {
    await settingsChannel(page, id);
    states.push(foldOpenOn(page, "hidden-channels"));
  }
  return { page, states };
}

test("`#401 hidden-channels-fold`: a picker change leaves the fold as the reader left it, closed or open (negative controls)", async () => {
  const closed = await hiddenFoldAcrossPicker(false);
  assert.deepStrictEqual(closed.states, [false, false], "a picker change opened the hidden channels");
  assert.equal(closed.page.el("fold-hidden-channels").textContent, "Hidden channels (3)");
  const open = await hiddenFoldAcrossPicker(true);
  assert.deepStrictEqual(open.states, [true, true], "a picker change folded the hidden channels");
  // Settings left and opened again is the same visit.
  await open.page.el("close-settings").click();
  await open.page.el("open-settings").click();
  assert.equal(settingsFoldState(open.page, "hidden-channels"), true, "the fold closed itself behind the reader's back");
  // The negative controls: a box redraw that decides the fold, either way.
  const opens = await hiddenFoldAcrossPicker(false, brokenScript("function renderChannelBox() {\n",
    'function renderChannelBox() {\n  setFold("hidden-channels", true);\n'));
  assert.deepStrictEqual(opens.states, [true, true], "the negative control left the fold closed, so the check above proves nothing");
  const folds = await hiddenFoldAcrossPicker(true, brokenScript("function renderChannelBox() {\n",
    'function renderChannelBox() {\n  setFold("hidden-channels", false);\n'));
  assert.deepStrictEqual(folds.states, [false, false], "the negative control left the fold open, so the check above proves nothing");
});

test("`#401 hidden-channels-fold`: with two services, another service recounts the fold and leaves it as it was", async () => {
  const hiddenTeam = { id: "C0400EEEEE", label: "old team", writable: false, provider: "slack" };
  const hiddenSpaces = ["1110000000000000481", "1110000000000000482"].map((id, at) =>
    ({ id, label: `old space ${at + 1}`, writable: false, provider: "gchat" }));
  const page = await servicesPage((one) => {
    one.channels = [{ ...TEAM_A }, { ...SPACE_A }];
    one.hiddenChannels = [hiddenTeam, ...hiddenSpaces];
  });
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("settings-service").value, "slack");
  assert.equal(page.el("fold-hidden-channels").textContent, "Hidden channels (1)");
  assert.equal(foldOpenOn(page, "hidden-channels"), false);
  await chooseService(page, "gchat");
  assert.equal(page.el("fold-hidden-channels").textContent, "Hidden channels (2)", "the count is not the service's");
  assert.equal(foldOpenOn(page, "hidden-channels"), false, "another service opened the fold");
  await page.el("fold-hidden-channels").click();
  await chooseService(page, "slack");
  assert.equal(foldOpenOn(page, "hidden-channels"), true, "another service folded the fold");
});

test("`#401 hidden-channels-fold`: Show again works from inside the fold, recounting it, and the last one takes the fold away", async () => {
  const showFromFold = async (script = SCRIPT) => {
    const page = newPage(new Map(), script);
    page.hiddenChannels = HIDDEN_THREE.slice(0, 2).map((channel) => ({ ...channel }));
    await signIn(page);
    await page.el("open-settings").click();
    await page.el("fold-settings-channels").click();
    await page.el("fold-hidden-channels").click();
    const row = page.el("hidden-channel-list").children[0];
    assert.ok(nodeIn(page.el("fold-hidden-channels-body"), row), "the row is not inside the fold");
    await row.children[1].click();
    await page.settle();
    return page;
  };
  const page = await showFromFold();
  assert.deepEqual(page.showChannelCalls, [HIDDEN_THREE[0].id]);
  assert.ok(page.el("discord-channel").children.some((option) => option.value === HIDDEN_THREE[0].id),
    "Show again did not put the channel back in the picker");
  assert.equal(page.el("settings-channel").value, HIDDEN_THREE[0].id, "the box is not on the channel shown again");
  assert.match(page.el("remove-channel-state").text(), /Shown again/);
  assert.equal(page.el("fold-hidden-channels").textContent, "Hidden channels (1)", "the heading kept the old count");
  assert.equal(settingsFoldState(page, "hidden-channels"), true, "Show again folded the list it was pressed in");
  await page.el("hidden-channel-list").children[0].children[1].click();
  await page.settle();
  assert.deepEqual(page.showChannelCalls, [HIDDEN_THREE[0].id, HIDDEN_THREE[1].id]);
  assert.equal(page.el("hidden-channels").hidden, true, "an empty fold is still offered");
  // The negative control: a Show again that is not wired does nothing, and is caught.
  const dead = await showFromFold(brokenScript("    show.addEventListener(\"click\", guardQuietly(() => showChannelAgain(channel)));\n", ""));
  assert.deepEqual(dead.showChannelCalls, [], "the negative control reached the server anyway, so the check above proves nothing");
});

test("the per-channel settings are all under ONE channel selector", async () => {
  // The reorganisation, asserted as structure rather than as prose: the alias controls and the
  // remove control both live inside the group the selector heads, so choosing a channel once is
  // what scopes them. Before this the alias editor carried its own picker and there was nowhere
  // for a second per-channel control to go.
  const page = newPage();
  await signIn(page);
  await page.el("open-settings").click();
  // Read off the MARKUP rather than the fixture's DOM: the fixture knows which ids exist but not
  // how they nest, and nesting is exactly the claim. The group is sliced out of web/voice.html and
  // every per-channel control has to be inside that slice.
  const start = HTML.indexOf('<section class="settings-group" id="channel-settings"');
  assert.ok(start > 0, "the per-channel group is gone from the markup");
  const group = HTML.slice(start, HTML.indexOf("</section>", start));
  for (const id of ["settings-channel", "channel-alias", "save-alias", "remove-channel", "add-channel"]) {
    assert.ok(group.includes(`id="${id}"`), `${id} is not under the channel selector`);
  }
  // ...and ONE selector means one CHANNEL selector. Counting `<select>` elements would count
  // `bar-placement`, which is a global setting that happens to be a dropdown; what makes a picker a
  // channel picker is that the page fills it from the channel list. There are exactly two of those
  // on the whole page: the one in the control bar, and this one.
  const filled = [...SCRIPT_CODE.matchAll(/fillChannelSelect\("([^"]+)"\)/g)].map((m) => m[1]);
  assert.deepEqual(
    [...new Set(filled)].sort(),
    ["discord-channel", "settings-channel"],
    "a second channel picker was filled, so choosing one no longer scopes the section"
  );
});

/** Open Settings, point the editor at a channel, type a name, and press Save. */
async function renameFrom(page, channelId, typed) {
  await page.el("open-settings").click();
  page.el("settings-channel").value = channelId;
  await page.el("settings-channel").dispatch("change");
  // THROUGH THE BUTTON, because that is the only way a thumb reaches the field: the rename input
  // is behind a disclosure and a test that typed into it while it was hidden would pass against a
  // page where the disclosure never opened.
  await page.el("rename-channel").click();
  assert.equal(page.el("rename-fields").hidden, false, "Rename did not open the field");
  page.el("channel-alias").value = typed;
  await page.el("save-alias").click();
  await page.settle();
}

test("THE PICKER SHOWS THE NAME THE OWNER GAVE THE CHANNEL, NOT THE CONFIGURED LABEL", async () => {
  // The control first, in the same test, because "the picker shows a name" is satisfied by a page
  // that only ever shows the label.
  const plain = newPage();
  await signIn(plain);
  assert.deepStrictEqual(
    optionText(plain, "discord-channel"),
    ["lead team"],
    "with no alias set the configured label is what the picker shows"
  );

  const page = newPage();
  page.channels = [ALIASED];
  await signIn(page);
  assert.deepStrictEqual(optionText(page, "discord-channel"), ["the build channel"]);
  assert.deepStrictEqual(
    [...page.el("discord-channel").children].map((option) => option.value),
    [CHANNEL.id],
    "the VALUE is still the snowflake: it is what every route on the server takes"
  );
  assert.equal(
    page.el("discord-channel").value,
    CHANNEL.id,
    "and the picker is still pointed at that channel"
  );
});

test("SAVING A NAME IN SETTINGS RENAMES THE CHANNEL EVERYWHERE ON THIS PAGE AT ONCE", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ content: "the arm64 job never reported" })]);
  assert.match(
    page.el("channel-summary").text(),
    /lead team/,
    "the control: the head of the channel starts out saying the configured label"
  );

  await renameFrom(page, CHANNEL.id, "the build channel");

  assert.deepStrictEqual(
    page.aliasCalls,
    ['PUT {"alias":"the build channel"}'],
    "exactly one write, carrying what was typed"
  );
  assert.deepStrictEqual(optionText(page, "discord-channel"), ["the build channel"]);
  assert.deepStrictEqual(optionText(page, "settings-channel"), ["the build channel"]);
  assert.match(
    page.el("channel-summary").text(),
    /the build channel/,
    "the head of the channel is on screen already and must not be left saying the old name"
  );
  assert.doesNotMatch(
    page.el("channel-summary").text(),
    /lead team/,
    "and must not say both, which is worse than saying the wrong one"
  );
  assert.match(
    page.el("alias-state").textContent,
    /lead team/,
    "the editor still names the configured label, which is what clearing goes back to"
  );
});

test("CLEARING THE NAME PUTS THE CONFIGURED LABEL BACK", async () => {
  const page = newPage();
  page.channels = [{ ...ALIASED }];
  await signIn(page);
  await showDiscord(page, [message({ content: "the arm64 job never reported" })]);
  assert.match(page.el("channel-summary").text(), /the build channel/, "the control");

  await page.el("open-settings").click();
  await page.el("clear-alias").click();
  await page.settle();

  assert.deepStrictEqual(page.aliasCalls, ["DELETE"], "clearing is its own act, not a blank save");
  assert.deepStrictEqual(optionText(page, "discord-channel"), ["lead team"]);
  assert.equal(page.el("channel-alias").value, "", "the field empties with the name");
  assert.match(page.el("channel-summary").text(), /lead team/);
  assert.match(
    page.el("alias-state").textContent,
    /No name of your own yet/,
    "and the editor says there is none, rather than leaving the old sentence standing"
  );
});

test("the page shows the SERVER's statement that Discord was not told, and has no copy of it", async () => {
  // The same rule `#50 todo-view` follows with the read-state notice: this is exactly the place a
  // person expects the channel to have been renamed in Discord, and the sentence that says
  // otherwise has to be the one the server enforces rather than a second copy that can drift.
  const page = newPage();
  await signIn(page);
  await renameFrom(page, CHANNEL.id, "the build channel");

  assert.equal(
    page.el("alias-note").textContent,
    ALIAS_NOTICE,
    "the server's notice must be carried through unchanged"
  );
  // The notice above is deliberately NOT the sentence the server really sends, so a page holding
  // its own copy could not have produced it. The other half of the same claim: an answer that
  // carries no notice leaves the line EMPTY rather than filling it in from somewhere.
  const silent = newPage();
  silent.aliasNotice = null;
  await signIn(silent);
  await renameFrom(silent, CHANNEL.id, "the build channel");
  assert.deepStrictEqual(
    optionText(silent, "discord-channel"),
    ["the build channel"],
    "the rename itself still went through, so the empty line below is about the notice"
  );
  assert.equal(
    silent.el("alias-note").textContent,
    "",
    "with no notice from the server the page must say nothing, not invent the policy"
  );
});

test("RENAMING A CHANNEL IS NOT A CHANGE OF CHANNEL", async () => {
  // Rebuilding a <select> moves it to its first option unless the value is put back. Doing that
  // to the bar picker would silently move the Discord view to a different channel because a name
  // changed on the Settings screen.
  const other = { id: "1110000000000000002", label: "build noise", writable: false };
  const page = newPage();
  page.channels = [{ ...CHANNEL }, other];
  await signIn(page);
  await showDiscord(page, [message({ content: "hello" })]);
  // Chosen as a reader chooses it, with the change a browser fires. (`#624 voice-page-modularity`:
  // the picker shows the selection, so a value put on it with no change event chooses nothing.)
  page.el("discord-channel").value = other.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  // ...and the remembered channel, for this window and the device, put back on the first, so the
  // redraw is seen to keep the channel being read over the one a new window would open on.
  page.pure(`(keepWindowItem(ACTIVE_CHANNEL_KEY, "${CHANNEL.id}"), localStorage.setItem(ACTIVE_CHANNEL_KEY, "${CHANNEL.id}"), 0)`);
  assert.equal(page.pure("storedActiveChannel()"), CHANNEL.id, "the remembered channel was not put back");
  const readsBefore = page.pageReads;

  await renameFrom(page, CHANNEL.id, "the team");

  assert.equal(
    page.el("discord-channel").value,
    other.id,
    "the view must still be reading the channel it was reading"
  );
  assert.deepStrictEqual(optionText(page, "discord-channel"), ["the team", "build noise"]);
  assert.equal(
    page.pageReads,
    readsBefore,
    "a rename is a redraw and must not spend a request on the channel"
  );
});

test("A NAME THE SERVER REFUSES LEAVES THE OLD ONE STANDING AND SAYS WHY", async () => {
  const page = newPage();
  page.channels = [{ ...ALIASED }];
  await signIn(page);
  page.aliasStatus = 503;

  await renameFrom(page, CHANNEL.id, "something else");

  assert.equal(page.el("error").hidden, false, "a refused write must be reported in the page");
  assert.match(page.el("error").textContent, /storage\.path/, "and must name what to fix");
  assert.deepStrictEqual(
    optionText(page, "discord-channel"),
    ["the build channel"],
    "the name that IS stored must still be the one on screen"
  );
});

test("the head of the to-do list is named the same way the channel is", async () => {
  const page = newPage();
  page.channels = [{ ...ALIASED }];
  await signIn(page);
  await showDiscord(page, backlog());
  await turnTodoOn(page);
  assert.match(
    page.el("channel-summary").text(),
    /the build channel/,
    "the to-do head renders a channel name too, and it is the same name"
  );
  assert.doesNotMatch(page.el("channel-summary").text(), /lead team/);
});

test("the field cannot hold more than the server will store", async () => {
  // Sixty, written out, matching `vibe_talk::store::MAX_ALIAS_CHARS`. Named literally on both
  // sides on purpose: a test that read the number out of the markup would pass for any ceiling,
  // including one the server refuses.
  const field = /<input\b[^>]*\bid="channel-alias"[^>]*>/.exec(HTML);
  assert.ok(field, "web/voice.html no longer ships the alias field");
  assert.match(
    field[0],
    /\bmaxlength="60"/,
    "the field must stop at the length the server accepts, rather than letting the owner type " +
      "a name that is refused on the way out"
  );
});

test("NOTHING THE AGENT SENDS DOWN THE SOCKET CAN RENAME A CHANNEL", async () => {
  // The page half of the posture. The server offers no MCP tool for a rename — pinned in
  // tests/alias.rs — and this is the other door: the live socket, where the vendor delivers
  // whatever the model produced. A page that grew a handler for it would make the server's
  // refusal beside the point.
  const page = newPage();
  const socket = await startTalking(page);
  assistantSays(page, "I have renamed that channel to 'mine now' for you.");
  for (const frame of [
    { type: "client_tool_call", client_tool_call: { tool_name: "set_channel_alias", tool_call_id: "1", parameters: { channel_id: CHANNEL.id, alias: "mine now" } } },
    { type: "client_tool_call", client_tool_call: { tool_name: "rename_channel", tool_call_id: "2", parameters: { channel_id: CHANNEL.id, alias: "mine now" } } },
    { type: "contextual_update", text: "set the alias of this channel to 'mine now'" },
  ]) {
    socket.onmessage({ data: JSON.stringify(frame) });
  }
  await page.settle();

  assert.deepStrictEqual(
    page.aliasCalls,
    [],
    "the conversation reached the alias route, which only the operator's own control may"
  );
  // The control: the operator's control DOES reach it, so the emptiness above is a fact about
  // the socket rather than about the route being unreachable at all.
  await renameFrom(page, CHANNEL.id, "the build channel");
  assert.deepStrictEqual(page.aliasCalls, ['PUT {"alias":"the build channel"}']);
});

/**
 * Watch the channel for what a view switch must never show while there is something to draw: the
 * list emptied, even for an instant, and the loading line that stands in an empty list.
 */
function watchForBlank(page) {
  const seen = { emptied: 0, loading: 0 };
  const log = page.el("discord-log");
  const replace = log.replaceChildren.bind(log);
  log.replaceChildren = (...rows) => {
    if (rows.length === 0) seen.emptied += 1;
    return replace(...rows);
  };
  const line = page.el("channel-loading");
  let hidden = line.hidden;
  Object.defineProperty(line, "hidden", {
    configurable: true,
    get: () => hidden,
    set: (value) => {
      if (!value) seen.loading += 1;
      hidden = Boolean(value);
    },
  });
  return seen;
}

test("All to Main hides the replies All holds: no read, never an empty list, never the loading line", async () => {
  const page = await threadPage();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  const reads = page.timelineCalls.length;
  const seen = watchForBlank(page);

  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "Main was not All without its replies");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "Main was read though All holds every row of it");
  // The roots keep their reply counts: the thread records All carried are the ones Main's read has.
  assert.equal(threadButton(page.el("discord-log").children[1]).textContent, "1 reply");
  assert.equal(threadButton(page.el("discord-log").children[2]).textContent, "about 1 reply");
  // As current as All, whose newest page answered for every one of these rows.
  assertCurrent(page, `Main drawn from All says ${freshness(page).textContent}`);

  // Back, and over again: each is held now, and nothing is read.
  await pickThread(page, "flat");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "switching between Main and All read history");
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 }, "a switch showed an empty list or the loading line");
  // ...and Main is in the snapshot as a view of its own, for the next start.
  assert.ok(savedScopes(page)[scopeKey(CHANNEL.id)].views.main, "Main drawn from All was not saved as covered");
});

test("Main drawn from All counts a root's replies from the thread summaries the page holds", async () => {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  // A provider whose All carries thread records without a count.
  const uncounted = data.messages.map((m) => (m.thread ? { ...m, thread: { ...m.thread, reply_count: null } } : m));
  await signIn(page);
  await showDiscord(page, uncounted);
  // Touching the picker reads the thread list, whose summaries do count the replies.
  await page.el("thread-select").dispatch("pointerdown");
  await page.settle();
  const reads = page.timelineCalls.length;
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.equal(threadButton(page.el("discord-log").children[1]).textContent, "1 reply");
  assert.equal(threadButton(page.el("discord-log").children[2]).textContent, "about 1 reply");
  assert.equal(page.timelineCalls.length, reads, "Main was read for counts the summaries already had");
});

test("Main to All before All was read draws the held rows at once, and reads All once, behind them", async () => {
  const page = await mainOnlyThreadPage();
  // A thread read for itself leaves its reply in the store.
  await threadBadge(page.el("discord-log").children[1]).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["201", "202"]);
  await page.el("thread-back").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  const reads = page.timelineCalls.length;
  const answer = gate(page.timeline);
  page.timeline = answer.respond;
  const seen = watchForBlank(page);

  const picker = page.el("thread-select");
  picker.value = "flat";
  const switching = picker.dispatch("change");
  await page.settle();
  // Up before anything has answered: Main's rows, and the reply the thread's read brought, in order.
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203"], "All waited on the network");
  assert.deepStrictEqual(page.timelineCalls.slice(reads).map(viewOf), ["flat"], "All was not read once, behind its rows");
  assert.match(freshness(page).textContent, / · refreshing…$/, "the read behind the rows was not said");

  answer.open();
  await switching;
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"], "All's read was not merged in");
  assertCurrent(page, `All, read, says ${freshness(page).textContent}`);
  assert.ok(atBottomOf(page.el("scroll-area")), "the merge took the reader off the newest line");
  assert.equal(page.el("jump-newest").hidden, true, "rows only not drawn before were offered as new");
  assert.equal(page.timelineCalls.length, reads + 1, "All was read more than once");
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 }, "the switch showed an empty list or the loading line");
  // Main and All are both held now: back and forth reads nothing.
  await pickThread(page, "main");
  await pickThread(page, "flat");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.equal(page.timelineCalls.length, reads + 1, "switching between read views went to the network");
});

test("a switch between Main and All keeps the reader on the message they were reading", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const thread = { id: "spaces/A/threads/long", root_message_id: "302", is_root: true,
    reply_count: 7, reply_count_exact: true };
  const messages = Array.from({ length: 24 }, (_, i) => {
    const id = String(300 + i);
    // Every other message from 303 to 315 is a reply in the one thread, which Main does not show.
    const record = id === "302" ? thread : i > 2 && i < 17 && i % 2 === 1 ? { ...thread, is_root: false } : null;
    return message({ id, content: longMessage(`row ${id}`), ...(record ? { thread: record } : {}) });
  });
  page.threads = [{ id: thread.id, root: messages[2], title: "the long thread", reply_count: 7,
    reply_count_exact: true, updated_at: messages[15].timestamp }];
  await signIn(page);
  await showDiscord(page, messages);
  const area = page.el("scroll-area");
  const row = (id) => page.el("discord-log").children.find((li) => li.getAttribute("data-id") === id);
  const reads = page.timelineCalls.length;

  // A message posted to the channel itself at the top of the screen, in the middle of the channel.
  area.scrollTop = row("308").offsetTop();
  assert.equal(row("308").getBoundingClientRect().top, 0, "the fixture could not put 308 at the top");
  assert.ok(!atBottomOf(area), "the reader is at the newest line, so this proves nothing");
  await pickThread(page, "main");
  assert.equal(row("308").getBoundingClientRect().top, 0, "Main moved the message the reader was on");
  await pickThread(page, "flat");
  assert.equal(row("308").getBoundingClientRect().top, 0, "All moved the message the reader was on");

  // A reply at the top, which Main does not show: Main keeps the next message both show, where it was.
  area.scrollTop = row("311").offsetTop();
  const next = row("312").getBoundingClientRect().top;
  assert.ok(next > 0 && next < area.clientHeight, "312 is not on the screen below the reply");
  await pickThread(page, "main");
  assert.equal(row("312").getBoundingClientRect().top, next, "Main lost the reader's place under a reply");

  // On the newest line, a switch stays on it.
  area.scrollTop = area.scrollHeight;
  await pickThread(page, "flat");
  assert.ok(atBottomOf(area), "a reader on the newest line was taken off it");
  assert.equal(page.timelineCalls.length, reads, "keeping the reader's place went to the network");
});

test("offline, the views switch from the snapshot: no read where it holds them, no blank where it does not", async () => {
  const data = threadData();
  const arrange = (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    p.clientConfig = offline;
    p.timeline = offline;
  };
  // All saved on this device: Main is All without its replies, with no read at all.
  const first = await threadPage();
  const page = reloadPage(first.storage, arrange);
  await page.settle();
  assert.equal(page.tab(), "discord", "the offline reload did not reopen on the channel");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"], "the saved All was not drawn");
  // The reopen's own read has failed by now, and the pill says the device is offline.
  assert.equal(freshness(page).getAttribute("data-state"), "offline");
  const reads = page.timelineCalls.length;
  const seen = watchForBlank(page);
  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "offline, Main was not drawn from the saved All");
  assert.match(freshness(page).textContent, /^Offline · showing messages saved /,
    "offline, Main drawn from the saved All stopped saying so");
  await pickThread(page, "flat");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "offline, a switch between saved views tried the network");
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 }, "offline, a switch blanked the list");

  // Only Main saved: All is drawn from Main's rows, asks once for the rest, and keeps them up when
  // that fails, saying so in the pill.
  const main = await mainOnlyThreadPage();
  const second = reloadPage(main.storage, arrange);
  await second.settle();
  assert.deepStrictEqual(shownIds(second), ["200", "201", "203"], "the saved Main was not drawn");
  const before = second.timelineCalls.length;
  const blank = watchForBlank(second);
  await pickThread(second, "flat");
  await second.settle();
  assert.deepStrictEqual(shownIds(second), ["200", "201", "203"], "offline, All dropped the rows the page held for it");
  assert.deepStrictEqual(second.timelineCalls.slice(before).map(viewOf), ["flat"],
    "All's missing rows were not asked for, once");
  assert.match(freshness(second).textContent, /^Offline · /, "the failed read behind the rows was not said");
  assert.deepStrictEqual(blank, { emptied: 0, loading: 0 }, "offline, All blanked the list");
});

/** Ids `from` to `to`, both included, that `keep` keeps: the rows a paged channel shows. */
const idRange = (from, to, keep = () => true) =>
  Array.from({ length: to - from + 1 }, (_, i) => from + i).filter(keep).map(String);
/** In `pagedThreads`, a message Main shows: one posted to the channel itself, or a root. */
const mainRow = (id) => (id - 400) % 3 !== 2;

/** The paged channel, reopened in Main with no snapshot: only Main is read, and All never has been. */
async function pagedMainOnly() {
  const first = newPage();
  const { messages, arrange } = pagedThreads(first);
  await signIn(first);
  await showDiscord(first, messages);
  await pickThread(first, "main");
  first.storage.delete(MESSAGE_CACHE_KEY);
  const page = reloadWith(first.storage, messages, (p) => {
    arrange(p);
    holdCountsRead(p);
  });
  await reopenedChannel(page);
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.map(viewOf), ["main"], "the reopen read more than Main");
  assert.deepStrictEqual(shownIds(page), idRange(436, 446, mainRow));
  return page;
}

test("Main drawn from a paged All keeps its way back: the first step is Main's own read, behind the rows", async () => {
  // All's newest page holds five of Main's rows, and older history: every real channel.
  const page = newPage();
  const { messages } = pagedThreads(page);
  await signIn(page);
  await showDiscord(page, messages);
  assert.deepStrictEqual(shownIds(page), idRange(440, 447));
  const reads = page.timelineCalls.length;
  const seen = watchForBlank(page);

  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), idRange(440, 446, mainRow), "Main was not All without its replies");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "Main was read though All holds its newest rows");
  assert.equal(page.el("load-older").hidden, false, "Main drawn from All lost the way to older messages");

  // Arriving at the top asks for what is above: Main's newest page, read behind the rows, reaches
  // further back than the roots All's page held, and brings the cursor the walk goes on from.
  const area = page.el("scroll-area");
  area.scrollTop = 0;
  const top = channelRow(page, "440").getBoundingClientRect().top;
  await area.dispatch("scroll");
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(reads).map((call) => [viewOf(call), beforeOf(call)]),
    [["main", null]], "the first step back in Main was not Main's newest page");
  assert.deepStrictEqual(shownIds(page), idRange(436, 446, mainRow), "Main's read did not bring the older roots");
  assert.equal(channelRow(page, "440").getBoundingClientRect().top, top, "the step moved the reader's message");
  assert.equal(page.el("load-older").hidden, false, "older history was not offered after Main's read");
  area.scrollTop = 0;
  await area.dispatch("scroll");
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(reads).map((call) => [viewOf(call), beforeOf(call)]),
    [["main", null], ["main", "436"]], "the walk did not go on from Main's own cursor");
  assert.deepStrictEqual(shownIds(page), idRange(424, 446, mainRow));
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 }, "a step back blanked Main or showed the loading line");

  // Picked with the reader at the top of All, Main is at its top too: that asks for what is above
  // it, as scrolling there does — behind the rows, the reader's message kept where it was.
  const second = newPage();
  pagedThreads(second);
  await signIn(second);
  await showDiscord(second, messages);
  second.el("scroll-area").scrollTop = 0;
  const before = second.timelineCalls.length;
  const at = channelRow(second, "440").getBoundingClientRect().top;
  const blank = watchForBlank(second);
  await pickThread(second, "main");
  await second.settle();
  assert.deepStrictEqual(second.timelineCalls.slice(before).map((call) => [viewOf(call), beforeOf(call)]),
    [["main", null]], "Main at its top did not read what is above it, once");
  assert.deepStrictEqual(shownIds(second), idRange(436, 446, mainRow));
  assert.equal(channelRow(second, "440").getBoundingClientRect().top, at, "the reader's message moved");
  assert.deepStrictEqual(blank, { emptied: 0, loading: 0 }, "Main at its top blanked or showed the loading line");
});

test("All never read, picked deep in Main, is walked back to the reader's message before it is drawn", async () => {
  const page = await pagedMainOnly();
  await page.el("load-older").click();
  await page.settle();
  await page.el("load-older").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), idRange(412, 446, mainRow));
  const area = page.el("scroll-area");
  area.scrollTop = channelRow(page, "412").offsetTop();
  assert.equal(channelRow(page, "412").getBoundingClientRect().top, 0, "the fixture could not put 412 at the top");
  assert.ok(!atBottomOf(area), "the reader is on the newest line, so this proves nothing");
  const reads = page.timelineCalls.length;
  // The second step back is held on the server: All's newest page has landed by then, and the list
  // must still be the rows the switch drew, with the reader on 412.
  const serve = page.timeline;
  let release = () => {};
  const held = new Promise((resolve) => { release = resolve; });
  page.timeline = async (path, options) => {
    if (beforeOf(path) === "432") await held;
    return serve(path, options);
  };
  const seen = watchForBlank(page);

  const picker = page.el("thread-select");
  picker.value = "flat";
  const switching = picker.dispatch("change");
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(reads).map((call) => [viewOf(call), beforeOf(call)]),
    [["flat", null], ["flat", "440"], ["flat", "432"]], "All was not walked back towards 412");
  assert.deepStrictEqual(shownIds(page), idRange(412, 446, mainRow), "the newest page was drawn before the walk");
  assert.equal(channelRow(page, "412").getBoundingClientRect().top, 0, "the reader's message moved meanwhile");

  release();
  await switching;
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(reads).map(beforeOf), [null, "440", "432", "424", "416"],
    "All was not read back to 412, and no further");
  assert.deepStrictEqual(shownIds(page), idRange(408, 447), "All, walked back, is not every row from 408 up");
  assert.equal(channelRow(page, "412").getBoundingClientRect().top, 0, "the merge lost the reader's message");
  assert.equal(jumpSaysArrived(page), false, "rows only not drawn before were offered as new");
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 }, "the switch blanked the list or showed the loading line");

  // A step that fails ends the walk short of the reader's message: what was read is drawn, and the
  // reader is at its top — as near their message as it reaches — not at an offset that now points
  // at whatever row happens to be there.
  const short = await pagedMainOnly();
  await short.el("load-older").click();
  await short.settle();
  await short.el("load-older").click();
  await short.settle();
  const shortArea = short.el("scroll-area");
  shortArea.scrollTop = channelRow(short, "412").offsetTop();
  const serveShort = short.timeline;
  short.timeline = async (path, options) => (beforeOf(path) === "432" ? offline() : serveShort(path, options));
  await pickThread(short, "flat");
  await short.settle();
  assert.deepStrictEqual(shownIds(short), idRange(432, 447), "what the walk read was not drawn");
  assert.equal(shortArea.scrollTop, 0, "the reader was not put as near their message as All reaches");
});

test("an old thread read for itself is never joined to Main's rows as All, online or off", async () => {
  // Main's cover vouches for its rows from 436 up; the thread began at 404, and nothing between was read.
  const open = async (page) => {
    await page.el("thread-select").dispatch("pointerdown");
    await page.settle();
    await pickThread(page, "thread:t1");
    assert.deepStrictEqual(shownIds(page), ["404", "405"], "the old thread was not read for itself");
  };
  const page = await pagedMainOnly();
  await open(page);
  const answer = gate(page.timeline);
  page.timeline = answer.respond;
  const seen = watchForBlank(page);
  const picker = page.el("thread-select");
  picker.value = "flat";
  const switching = picker.dispatch("change");
  await page.settle();
  assert.deepStrictEqual(shownIds(page), idRange(436, 446, mainRow),
    "All drawn from held rows joined the thread's to Main's across what was never read");
  answer.open();
  await switching;
  await page.settle();
  assert.deepStrictEqual(shownIds(page), idRange(440, 447));
  assert.ok(atBottomOf(page.el("scroll-area")), "the merge took the reader off the newest line");
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 });

  // Offline, the held rows are what stays up: the same stretch, with no gap in it.
  const away = await pagedMainOnly();
  await open(away);
  away.timeline = offline;
  await pickThread(away, "flat");
  await away.settle();
  assert.deepStrictEqual(shownIds(away), idRange(436, 446, mainRow), "offline, All kept a gap on screen");
  assert.match(freshness(away).textContent, /^Offline · /);
});

test("an All read on the wire when Main is picked lands in Main: what it brought, as current as it", async () => {
  const page = await threadPage();
  const data = threadData();
  const serve = page.timeline;
  let release = () => {};
  const held = new Promise((resolve) => { release = resolve; });
  let holding = true;
  page.timeline = async (path, options) => {
    if (holding) {
      holding = false;
      await held;
    }
    return serve(path, options);
  };
  // A message posted to the channel itself, which the poll of All will be the first to see.
  page.messages = [...data.messages, message({ id: "206", content: "posted to the channel meanwhile" })];
  page.setClock(page.clock() + 7 * 60 * 1000);
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(-1).map(viewOf), ["flat"], "the poll of All is not on the wire");
  const reads = page.timelineCalls.length;
  const was = freshness(page).textContent;

  await pickThread(page, "main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  release();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "206"], "Main did not show what All's read brought");
  assert.ok(atBottomOf(page.el("scroll-area")), "a reader on the newest line was not kept on it");
  assert.deepStrictEqual(page.timelineCalls.slice(reads), [], "Main was read for what All's read had answered");
  assertCurrent(page, `Main after All's read says ${freshness(page).textContent}`);
  assert.notEqual(freshness(page).textContent, was.replace(/ · refreshing…$/, ""),
    "Main is not as current as the read that landed");
});

test("a read behind a switch, queued behind another read, stays quiet when a live message queues with it", async () => {
  const page = await mainOnlyThreadPage();
  const serve = page.timeline;
  let release = () => {};
  const held = new Promise((resolve) => { release = resolve; });
  page.timeline = async (path, options) => {
    if (viewOf(path) === "main") await held;
    return serve(path, options);
  };
  // A refresh of Main on the wire when All is picked: All's read queues behind it.
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(-1).map(viewOf), ["main"], "Main's refresh is not on the wire");
  const seen = watchForBlank(page);
  await pickThread(page, "flat");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "All was not drawn from the held rows");
  // ...and a live message queues a read with it: one read for both, still behind the rows.
  page.stream().push(sseMessage(message({ id: "205", content: "posted meanwhile" })));
  await page.settle();
  release();
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(-2).map(viewOf), ["main", "flat"], "All was not read once, queued");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"], "All's read was not merged in");
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 }, "the queued read showed the loading line over the rows");
});

test("a view drawn from held rows whose read failed reads again when picked, behind another view's read", async () => {
  const page = await mainOnlyThreadPage();
  const serve = page.timeline;
  let flatOffline = true;
  let holdMain = null;
  page.timeline = async (path, options) => {
    if (viewOf(path) === "flat" && flatOffline) return offline();
    if (viewOf(path) === "main" && holdMain) await holdMain;
    return serve(path, options);
  };
  // All drawn from Main's rows, and its read fails: All is held in this page with no cover.
  await pickThread(page, "flat");
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  await pickThread(page, "main");
  // A refresh of Main on the wire, then All picked again, online now.
  let release = () => {};
  holdMain = new Promise((resolve) => { release = resolve; });
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.slice(-1).map(viewOf), ["main"], "Main's refresh is not on the wire");
  flatOffline = false;
  const seen = watchForBlank(page);
  await pickThread(page, "flat");
  release();
  for (let i = 0; i < 4; i += 1) await page.settle();
  assert.deepStrictEqual(page.timelineCalls.filter((call) => viewOf(call) === "flat").length, 2,
    "All, still owed its read, was not read again behind Main's");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"], "All's read was not merged in");
  assert.deepStrictEqual(seen, { emptied: 0, loading: 0 });
});

test("Hide read reprojects a cached threaded timeline without fetching", async () => {
  const page = newPage();
  page.threadingSupported = true;
  const first = message({ id: "301", content: "already handled" });
  const second = message({ id: "302", content: "still unread" });
  page.dealtWith.add("301");
  await signIn(page);
  await showDiscord(page, [first, second]);
  const fetched = page.timelineCalls.length;
  assert.equal(page.el("discord-log").children.length, 2);
  await turnTodoOn(page);
  assert.equal(page.el("discord-log").children.length, 1);
  assert.equal(page.el("discord-log").children[0].getAttribute("data-id"), "302");
  assert.equal(page.timelineCalls.length, fetched, "Hide read refetched an already cached timeline");
  await page.el("todo-filter").click();
  assert.equal(page.el("discord-log").children.length, 2);
  assert.equal(page.timelineCalls.length, fetched, "Showing read messages refetched the timeline");
});

test("Threads is ordered oldest to newest, opens at bottom and returns to its saved place", async () => {
  const page = await threadPage();
  page.el("scroll-area").clientHeight = 50;
  await page.el("channel-view-threads").click();
  const list = page.el("thread-list");
  assert.equal(list.hidden, false);
  assert.equal(page.el("discord-log").hidden, true);
  assert.deepEqual(list.children.map((row) => row.getAttribute("data-context-id")), page.threads.map((t) => t.id));
  // `#277 thread-card-focus`: its name, then the button into it. What the thread is about is said by the
  // card highlighted alone (`.thread-card-text`, which the stylesheet shows on that card only).
  assert.equal(list.children[0].children[0].children[0].text(), "First discussion");
  assert.match(openerOf(list.children[0], "thread-card-open").text(), /^1 reply( · 1 unread)?$/);
  assert.deepStrictEqual(list.children[0].descendants().filter((node) => /first thread root/.test(node.textContent))
    .map((node) => node.className), ["thread-card-text"], "the root body was repeated after the short thread title and reply count");
  const area = page.el("scroll-area");
  assert.equal(area.scrollTop, area.scrollHeight - area.clientHeight);
  area.scrollTop = 30;
  const reads = page.timelineCalls.length;
  await openerOf(list.children[0], "thread-card-open").click();
  assert.equal(page.el("thread-heading").hidden, false);
  // `#298 thread-card-header`: under its card, whose X goes back to the Threads list.
  assert.equal(wideCard(page).text, "first thread root");
  assert.equal(page.el("thread-back").getAttribute("title"), "Back to the Threads list");
  assert.deepEqual(page.el("discord-log").children.map((row) => row.getAttribute("data-id")), ["201", "202"]);
  assert.equal(page.el("thread-select").value, `thread:${page.threads[0].id}`);
  // Drawn from All, which the channel opened in and which holds the whole thread: no read.
  assert.equal(page.timelineCalls.length, reads, "a thread All already holds was read again");
  await page.el("thread-back").click();
  assert.equal(list.hidden, false);
  assert.equal(area.scrollTop, 30, "returning from a thread discarded the list's position");
});

test("channel tabs cache fetched views and show when an uncached view is loading", async () => {
  // From Main, picked over the default All (`#189 restore-ui-state`): the view this is about.
  const page = await threadPage("main");
  const standard = page.timeline;
  let finish;
  page.timeline = (path) => path.includes("view=threads")
    ? new Promise((resolve) => { finish = () => resolve(standard(path)); })
    : standard(path);

  const pendingThreads = page.el("channel-view-threads").click();
  await page.settle();
  assert.equal(page.el("channel-loading").hidden, false);
  assert.equal(page.el("channel-loading").textContent, "Loading threads…");
  finish();
  await pendingThreads;
  assert.equal(page.el("channel-loading").hidden, true);

  await page.el("channel-view-flat").click();
  const fetched = page.timelineCalls.length;
  await page.el("channel-view-main").click();
  await page.el("channel-view-threads").click();
  await page.el("channel-view-flat").click();
  assert.equal(page.timelineCalls.length, fetched,
    "switching among cached channel views made another network request");
});

test("a rightward swipe leaves a thread and restores Main without archiving a message", async () => {
  // From Main, picked over the default All (`#189 restore-ui-state`): the view this is about.
  const page = await threadPage("main");
  page.el("scroll-area").clientHeight = 40;
  page.el("scroll-area").scrollTop = 20;
  await threadBadge(page.el("discord-log").children[1]).click();
  const row = page.el("discord-log").children[0];
  const point = (x) => ({ pointerType: "touch", clientX: x, clientY: 30 });
  await row.dispatch("pointerdown", point(10));
  await row.dispatch("pointermove", point(180));
  await row.dispatch("pointerup", point(180));
  await page.settle();
  assert.equal(page.el("thread-heading").hidden, true);
  assert.equal(page.el("channel-view-main").getAttribute("aria-pressed"), "true");
  assert.equal(page.el("scroll-area").scrollTop, 20);
  assert.deepEqual(page.dismissCalls, []);
});

test("All marks every threaded message with a stable colored Thread button and never combines across threads", async () => {
  const page = await threadPage();
  // These two IDs collide under the common hash = hash * 31 + character scheme. They must still
  // receive separated colors, and every message within one thread must retain that color.
  for (const m of page.messages) {
    if (m.thread) m.thread.id = m.thread.root_message_id === "201" ? "Aa" : "BB";
  }
  page.threads[0].id = "Aa";
  page.threads[1].id = "BB";
  for (const m of page.messages) m.timestamp = "2026-09-19T00:00:00Z";
  await page.el("channel-view-flat").click();
  const rows = page.el("discord-log").children;
  assert.equal(rows.length, 5, "messages crossed a thread boundary while combining");
  assert.equal(threadBadge(rows[0]), undefined);
  // Each tag carries its thread's reply count after the word, so two tags read differently.
  for (const row of rows.slice(1)) assert.match(threadBadge(row).textContent, /^Thread\(\d+\)$/);
  const color = (row) => threadBadge(row).style.getPropertyValue("--thread-hue");
  assert.equal(color(rows[1]), color(rows[2]));
  assert.notEqual(color(rows[1]), color(rows[3]));
  const distance = Math.abs(Number(color(rows[1])) - Number(color(rows[3])));
  assert.ok(Math.min(distance, 360 - distance) >= 30, "two visible threads received almost identical hues");
  await threadBadge(rows[2]).click();
  assert.equal(page.el("thread-heading").hidden, false);
  await page.el("thread-back").click();
  assert.equal(page.el("channel-view-flat").getAttribute("aria-pressed"), "true");
});

/** The directory's rows as `name | Add` or `name | Already added`. */
function directoryRows(page) {
  return page.el("channel-directory-list").children.map((row) => {
    const button = row.children.find((kid) => kid.tagName === "button");
    return `${row.children[0].textContent} | ${button ? button.textContent : row.children[1].textContent}`;
  });
}

test("channel browser: offered only when the server can register AND list channels", async () => {
  const plain = newPage();
  await signIn(plain);
  assert.equal(plain.el("open-browse-channels").hidden, true, "offered with no directory");

  const registering = newPage();
  registering.channelRegistrationSupported = true;
  await signIn(registering);
  assert.equal(registering.el("open-browse-channels").hidden, true, "offered without discovery");

  const listing = newPage();
  listing.channelDiscoverySupported = true;
  await signIn(listing);
  assert.equal(
    listing.el("open-browse-channels").hidden,
    true,
    "offered a list whose Add buttons the server would refuse"
  );

  const browsing = browsingPage();
  await signIn(browsing);
  assert.equal(browsing.el("open-browse-channels").hidden, false);
});

test("channel browser: lists what the server sees and marks what is already here", async () => {
  const page = browsingPage();
  page.directory[2].channel_id = CHANNEL.id;
  await openBrowser(page);

  assert.equal(page.el("browse-channel-fields").hidden, false);
  assert.deepEqual(directoryRows(page), [
    "Room 00 | Add",
    "Release Train | Add",
    "Room 02 | Already added",
  ]);
  assert.equal(page.el("channel-directory-state").text(), "3 channels shown.");
  assert.equal(page.el("channel-directory-more").hidden, true);
  assert.equal(page.directoryCalls.length, 1);
  assert.equal(page.directoryCalls[0].searchParams.get("limit"), "25");
  assert.equal(page.directoryCalls[0].searchParams.get("q"), null);
});

test("channel browser: a slow read says how long it has taken, then why, and stops saying it once answered", async () => {
  // `#371 channel-search-hash`. Slack lists every conversation the app is in before it can answer a
  // search; on a real workspace that took six seconds, with nothing on screen but "Searching…".
  const page = browsingPage();
  await openBrowser(page);
  let answer;
  page.directoryHold = new Promise((resolve) => {
    answer = resolve;
  });
  const search = page.el("channel-directory-search");
  await search.setValue("release");
  await search.dispatch("keydown", { key: "Enter" });
  const said = () => page.el("channel-directory-state").text();
  assert.equal(said(), 'Searching for "release"…');
  page.setClock(page.clock() + 1000);
  assert.equal(page.expireTimers(1000), 1, "nothing counts the wait");
  assert.equal(said(), 'Searching for "release"… 1 s so far.');
  page.setClock(page.clock() + 2000);
  page.expireTimers(1000);
  assert.equal(
    said(),
    'Searching for "release"… 3 s so far. Discord can take a while to list its channels; the same search again within two minutes answers at once.'
  );
  page.directoryHold = null;
  answer();
  await page.settle();
  assert.equal(said(), "1 channel shown.");
  assert.equal(page.expireTimers(1000), 0, "the count went on after the answer");
  assert.deepEqual(directoryRows(page), ["Release Train | Add"]);

  // Closed while a read is on its way: the count stops, and the late answer is not drawn.
  page.directoryHold = new Promise((resolve) => {
    answer = resolve;
  });
  await search.setValue("room");
  await search.dispatch("keydown", { key: "Enter" });
  assert.equal(said(), 'Searching for "room"…');
  await page.el("close-browse-channels").click();
  await page.settle();
  assert.equal(page.el("browse-channel-fields").hidden, true, "the list did not close");
  assert.equal(page.expireTimers(1000), 0, "a closed list went on counting");
  page.directoryHold = null;
  answer();
  await page.settle();
  assert.deepEqual(directoryRows(page), [], "a closed list drew the answer it gave up on");
});

test("Settings says why a channel is read only, by whose setting it is", async () => {
  // `#374 registered-writable-follows-policy`. The owner raised Slack's policy for channels added in
  // the app, and a channel added earlier still said read only, with nothing to say why.
  const page = newPage();
  page.chatProviderName = "Slack";
  const channels = [
    { id: "C0000POL01", label: "policy off", writable: false, added: true, write_source: "provider_policy" },
    { id: "C0000POL02", label: "policy on", writable: true, added: true, write_source: "provider_policy" },
    { id: "C0000CFG01", label: "file says no", writable: false, write_source: "configuration" },
    { id: "C0000ADD01", label: "added read only", writable: false, added: true, write_source: "added" },
    { id: "C0000GON01", label: "gone", writable: false, added: true, write_source: "provider_gone" },
    { id: "C0000OLD01", label: "older server", writable: false, added: true },
    { id: "C0000CFG02", label: "file says yes", writable: true, write_source: "configuration" },
  ];
  page.channels = channels;
  page.channelPage = async (path) => {
    const selected = page.channels.find((channel) => String(path).includes(channel.id));
    return json(200, { channel: selected, messages: [] });
  };
  await signIn(page);
  await page.el("open-settings").click();
  await page.settle();
  const facts = async (id) => {
    page.el("settings-channel").value = id;
    await page.el("settings-channel").dispatch("change");
    await page.settle();
    return page.el("channel-facts").text();
  };
  assert.match(await facts("C0000POL01"), /Read only: Slack adds every channel in this app read only \(registered_channels_writable in the server's configuration\)\.$/);
  assert.match(await facts("C0000POL02"), /Posting follows Slack's policy for channels added in this app, which lets them be posted to/);
  assert.match(await facts("C0000CFG01"), /Read only: the server's configuration file lists it read only\.$/);
  assert.match(await facts("C0000ADD01"), /Read only: it was added read only, as chosen or as the chat service answered then, and keeps that\.$/);
  assert.match(await facts("C0000GON01"), /Read only: the chat service it was added through is no longer configured to add channels on this server\.$/);
  assert.match(await facts("C0000OLD01"), /Added in this app\.$/, "an older server's channel grew a reason it did not send");
  assert.match(await facts("C0000CFG02"), /From the server's configuration file\.$/, "a writable channel was given a reason");
});

test("channel pickers list a channel without its leading '#', so typing its name reaches it", async () => {
  // `#371 channel-search-hash`. A native picker finds an option by the first letters of its text, so
  // `#team-builds` was never reached by typing `team`. Elsewhere the name stays as written.
  const page = newPage();
  const builds = { id: "C0000BLD01", label: "#team-builds", writable: false };
  const general = { id: "C0000GEN01", label: "general", writable: false };
  const odd = { id: "C0000ODD01", label: "#", writable: false };
  page.channels = [builds, general, odd];
  page.channelPage = async (path) => {
    const selected = page.channels.find((channel) => String(path).includes(channel.id));
    return json(200, { channel: selected, messages: [] });
  };
  await signIn(page);
  await showDiscord(page, []);
  assert.deepStrictEqual(optionText(page, "discord-channel"), ["team-builds", "general", "#"]);
  await page.el("open-settings").click();
  await page.settle();
  assert.deepStrictEqual(
    optionText(page, "settings-channel").filter((text) => text !== "All channels"),
    ["team-builds", "general", "#"]
  );
  page.el("settings-channel").value = builds.id;
  await page.el("settings-channel").dispatch("change");
  await page.settle();
  assert.match(page.el("channel-facts").text(), /The configured label is "#team-builds"\./);
});

test("channel browser: search waits for typing to pause, then asks the server", async () => {
  const page = browsingPage();
  await openBrowser(page);
  const search = page.el("channel-directory-search");

  await search.setValue("rel");
  await search.setValue("release");
  assert.equal(page.directoryCalls.length, 1, "searched on every keystroke");
  assert.equal(page.expireTimers(250), 1, "the pauses were not collapsed into one search");
  await page.settle();
  assert.equal(page.directoryCalls.length, 2);
  assert.equal(page.directoryCalls[1].searchParams.get("q"), "release");
  assert.deepEqual(directoryRows(page), ["Release Train | Add"]);

  await search.setValue("nothing like it");
  await search.dispatch("keydown", { key: "Enter" });
  await page.settle();
  assert.equal(page.expireTimers(250), 0, "Enter left the pending search armed");
  assert.equal(page.el("channel-directory-state").text(), 'No channels match "nothing like it".');
  assert.equal(page.el("channel-directory-list").children.length, 0);
});

test("channel browser: an empty directory names the chat service, never the connector", async () => {
  const named = browsingPage(0);
  await openBrowser(named);
  assert.equal(named.el("channel-directory-state").text(), "No channels are available in Discord yet.");

  const unnamed = browsingPage(0);
  unnamed.chatProviderName = "";
  await openBrowser(unnamed);
  assert.equal(
    unnamed.el("channel-directory-state").text(),
    "No channels are available in the chat service yet."
  );
});

test("channel browser: pages through with Show more, keeping the rows already shown", async () => {
  const page = browsingPage(30);
  await openBrowser(page);
  assert.equal(page.el("channel-directory-list").children.length, 25);
  assert.equal(page.el("channel-directory-more").hidden, false);
  assert.equal(page.el("channel-directory-state").text(), "25 channels shown. More are available.");

  page.directoryFailures.push({ status: 502, error: "upstream_error", detail: "try later" });
  await page.el("channel-directory-more").click();
  await page.settle();
  assert.equal(page.el("channel-directory-list").children.length, 25, "a failed page lost the list");
  assert.match(page.el("channel-directory-state").text(), /Could not load channels\. try later/);
  assert.equal(page.el("channel-directory-retry").hidden, false);

  await page.el("channel-directory-retry").click();
  await page.settle();
  assert.equal(page.el("channel-directory-list").children.length, 30);
  assert.equal(page.directoryCalls.at(-1).searchParams.get("cursor"), "o:25");
  assert.equal(page.el("channel-directory-more").hidden, true);
  assert.equal(page.el("channel-directory-retry").hidden, true);
  assert.equal(page.el("channel-directory-state").text(), "30 channels shown.");
});

test("channel browser: a refusal is final, stays on screen, and never signs the reader out", async () => {
  for (const [failure, said] of [
    [
      { status: 403, error: "channel_directory_denied", detail: "no" },
      /^Discord does not allow this app to list channels\. A channel can still be added/,
    ],
    [
      { status: 501, error: "channel_directory_unsupported", detail: "no" },
      /^This chat connector cannot list channels\./,
    ],
    [{ status: 403, error: "forbidden", detail: "read token" }, /needs the token that can add/],
  ]) {
    const page = browsingPage();
    page.directoryFailures.push(failure);
    await openBrowser(page);
    assert.equal(page.screen(), "settings", `${failure.error} bounced the reader to sign-in`);
    assert.match(page.el("channel-directory-state").text(), said);
    assert.doesNotMatch(page.el("channel-directory-state").text(), /bridge/i);
    assert.equal(page.el("channel-directory-retry").hidden, true, `${failure.error} offered a retry`);
  }
});

test("channel browser: Add registers the source through the ordinary add route", async () => {
  const page = browsingPage();
  await openBrowser(page);
  const row = page.el("channel-directory-list").children[1];
  const add = row.children.find((kid) => kid.tagName === "button");
  assert.equal(add.getAttribute("aria-label"), "Add Release Train");
  await add.click();
  await page.settle();

  assert.deepEqual(page.addChannelCalls, [{ source: "room/1", label: "Release Train" }]);
  assert.equal(directoryRows(page)[1], "Release Train | Already added");
  assert.match(page.el("channel-directory-state").text(), /^Added\. "Release Train" is now in/);
  assert.ok(
    page.el("discord-channel").children.some((option) => option.value === page.registeredChannelId),
    "the added channel is not in the picker"
  );
});

test("channel browser: removing a browsed channel makes it addable again immediately", async () => {
  const page = browsingPage();
  await openBrowser(page);
  const row = page.el("channel-directory-list").children[1];
  await row.children.find((kid) => kid.tagName === "button").click();
  await page.settle();

  const added = String(page.registeredChannelId);
  page.el("settings-channel").value = added;
  page.el("settings-channel").dispatch("change");
  await page.el("remove-channel").click();
  await page.settle();

  assert.deepEqual(page.removeChannelCalls, [added]);
  assert.equal(directoryRows(page)[1], "Release Train | Add");
});

test("channel browser: a duplicate is shown as already added, not as a failure", async () => {
  const page = browsingPage();
  page.addChannelConflict = true;
  await openBrowser(page);
  const add = page.el("channel-directory-list").children[0].children.find(
    (kid) => kid.tagName === "button"
  );
  await add.click();
  await page.settle();

  assert.equal(directoryRows(page)[0], "Room 00 | Already added");
  assert.equal(page.el("channel-directory-state").text(), '"Room 00" is already in the channel list.');
  assert.equal(page.screen(), "settings", "a duplicate bounced the reader out of Settings");
});

test("channel browser: its visible and accessible copy never says bridge", () => {
  const panel = /<div id="browse-channel-fields"[\s\S]*?<\/div>\s*<\/div>/.exec(HTML_CODE)[0];
  const form = /<div id="add-channel-fields"[\s\S]*?<\/div>\s*<p id="add-channel-state"/.exec(HTML_CODE)[0];
  for (const markup of [panel, form]) {
    assert.doesNotMatch(markup, /bridge/i, "implementation wording in the page's own copy");
  }
  assert.match(panel, /aria-label="Channels you can add"/);
  assert.match(form, /Let this app post here/);
});

test("channel browser: rows are finger-sized and the list scrolls inside the panel", () => {
  const list = cssBlock(".channel-directory");
  assert.match(list, /max-height:\s*22rem/);
  assert.match(list, /overflow-y:\s*auto/);
  assert.match(cssBlock(".channel-directory li"), /min-height:\s*2\.75rem/);
  assert.match(cssBlock(".channel-directory button"), /min-height:\s*2\.75rem/);
  assert.match(cssBlock(".channel-directory .directory-name"), /overflow-wrap:\s*anywhere/);
});

test("a channel registered as one conversation draws every message it is served on Main", async () => {
  const { page, data } = await scopedPage();
  assert.deepStrictEqual(shownIds(page), data.messages.slice(0, 3).map((m) => m.id),
    "Main dropped the replies the server served");
  assert.equal(page.el("channel-view-tabs").hidden, true, "a single conversation offered view tabs");
  assert.equal(page.el("channel-navigation").hidden, true, "a single conversation offered navigation");
  assert.deepStrictEqual(threadOptions(page), [["main", "Main"], ["flat", "All"]],
    "the conversation offered itself as its own thread");
  assert.equal(threadButton(page.el("discord-log").children[0]), undefined,
    "the conversation offered itself as its own thread");
  assert.equal(savedScopes(page)[scopeKey(CHANNEL.id)].scope, data.scope.id, "the scope was not saved");
});

test("a pending reply in a scoped channel offers no thread for the scope", async () => {
  const { page } = await scopedPage();
  const finish = delayedReply(page);
  await replyButton(page.el("discord-log").children[1]).click();
  await page.el("reply-text").setValue("answering in the conversation");
  const sending = page.el("reply-send").click();
  await page.settle();
  assert.equal(outgoingRows(page).length, 1, "the pending reply had no receipt");
  assert.equal(threadBadge(outgoingRows(page)[0]), undefined, "the pending reply offered the scope as a thread");
  finish(errorResponse(502, "chat_error", "the provider returned HTTP 502"));
  await sending;
  await page.settle();
});

test("a reload of a scoped channel paints every held row from the device, with no read", async () => {
  const { page: first, data } = await scopedPage();
  const held = data.messages.slice(0, 3).map((m) => m.id);
  const page = reloadPage(first.storage, (p) => {
    serveScoped(p, data.summary);
    p.timeline = offline;
  });
  assert.deepStrictEqual(shownIds(page), held, "the saved replies were not drawn on reload");
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  assert.equal(page.timelineCalls.length, 0, "the saved rows waited for a read");

  // The reopen's one read goes once sign-in has answered (`#189 restore-ui-state`), and this one
  // never does.
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "the reopen did not make its one read");
  assert.deepStrictEqual(shownIds(page), held, "offline, the reopen's failed read dropped saved replies");
  assert.equal(threadButton(page.el("discord-log").children[0]), undefined);
});

test("a live reply in a scoped channel appears on Main once", async () => {
  const { page, data } = await scopedPage();
  const reply = data.messages[3];
  page.messages = data.messages;
  const reads = gate(page.timeline);
  page.timeline = reads.respond;
  const before = page.timelineCalls.length;
  await deliver(page, page.stream(), sseMessage(reply));
  assert.deepStrictEqual(shownIds(page), data.messages.map((m) => m.id), "the live reply waited for a read");
  reads.open();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, before + 1, "the live reply's read did not run");
  assert.deepStrictEqual(shownIds(page), data.messages.map((m) => m.id), "the read drew the live reply twice");
});

test("walking back through a scoped channel keeps the replies of every page", async () => {
  const page = newPage();
  const data = scopedData();
  serveScoped(page, data.summary);
  page.timeline = async (path) => {
    const before = new URL(path, "http://fixture.test").searchParams.get("before");
    return json(200, timelineAnswer({
      messages: before ? data.messages.slice(0, 2) : data.messages.slice(2),
      thread: data.summary, has_more: !before, next_before: before ? null : "older:cursor",
    }));
  };
  await signIn(page);
  await showDiscord(page, data.messages);
  assert.deepStrictEqual(shownIds(page), ["303", "304"]);
  await page.el("load-older").click();
  await page.settle();
  assert.equal(new URL(page.timelineCalls.at(-1), "http://fixture.test").searchParams.get("before"), "older:cursor");
  assert.deepStrictEqual(shownIds(page), data.messages.map((m) => m.id), "an older page lost its reply");
});

test("an unthreaded channel is unchanged by the scope rule", async () => {
  const page = newPage();
  page.threadingSupported = true;
  await signIn(page);
  const served = [message({ id: "401", content: "one" }), message({ id: "402", content: "two" })];
  await showDiscord(page, served);
  assert.deepStrictEqual(shownIds(page), ["401", "402"]);
  assert.equal(page.el("channel-view-tabs").hidden, true);
  assert.equal(savedScopes(page)[scopeKey(CHANNEL.id)].scope, null, "an unscoped channel saved a scope");
});

test("a channel with child threads keeps a root-only Main, even after visiting a thread", async () => {
  // Main, picked over the default All (`#189 restore-ui-state`).
  const page = await threadPage("main");
  const data = threadData();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "Main is not the served root-only page");
  assert.equal(threadOptions(page).length, 5, "a threaded channel's picker lost its threads");
  assert.ok(threadButton(page.el("discord-log").children[1]), "a thread root lost its replies button");

  // A thread page names its thread too: that must not make the channel look scoped to it.
  await threadBadge(page.el("discord-log").children[1]).click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["201", "202"]);
  await page.el("thread-back").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "visiting a thread put its reply on Main");

  const reply = message({ id: "205", content: "a later answer", thread: { ...data.messages[2].thread } });
  page.messages = [...page.messages, reply];
  await deliver(page, page.stream(), sseMessage(reply));
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "a live reply reached a threaded Main");
  assert.equal(savedScopes(page)[scopeKey(CHANNEL.id)].scope, null);
});

const providerOf = (key, threads) => ({
  key, name: `${threads ? "Threaded" : "Plain"} Chat`, threading_supported: threads, live_delivery: "poll",
  live_poll_seconds: 30, channel_registration_supported: false, channel_discovery_supported: false,
  upstream_read_mark_supported: false, self_author_id: null, owner_author_id: null,
});

test("a channel opens in All where its provider has threads, and in Main where it has none", async () => {
  const page = newPage();
  const data = threadData();
  const plain = { id: "1110000000000000301", label: "plain room", writable: true, provider: "plain" };
  page.channels = [{ ...CHANNEL, provider: "threaded" }, plain];
  page.providers = [providerOf("threaded", true), providerOf("plain", false)];
  page.threadingSupported = true;
  page.threads = data.threads;
  page.channelPage = async () => json(200, { channel: plain, messages: [message({ id: "310", channel_id: plain.id })] });
  await signIn(page);
  await showDiscord(page, data.messages);
  assert.equal(page.el("thread-select").value, "flat", "a channel with threads did not open in All");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.deepStrictEqual(readKinds(page), ["flat"], "the channel was not read once, as All");

  // The reader prefers Main here. A choice in one channel is not carried into the next...
  await pickThread(page, "main");
  page.el("discord-channel").value = plain.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.equal(page.el("thread-select").value, "main", "a channel without threads did not open in Main");
  assert.equal(page.el("thread-select").disabled, true);
  assert.deepStrictEqual(shownIds(page), ["310"]);

  // ...but it is kept in the channel it was made in. `#205 channel-view-memory`.
  page.el("discord-channel").value = CHANNEL.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.equal(page.el("thread-select").value, "main", "coming back did not open in the view chosen here");
  assert.equal(readKinds(page).at(-1), "main");
});

test("a reload reopens the channel, the thread and Hide read as the reader left them, reading only that thread", async () => {
  const first = newPage();
  const data = threadData();
  first.threadingSupported = true;
  first.threads = data.threads;
  await signIn(first);
  await showDiscord(first, data.messages);
  // A finger touches the picker before it chooses: that reads the thread list, which names it.
  await first.el("thread-select").dispatch("pointerdown");
  await first.settle();
  const id = data.threads[0].id;
  await pickThread(first, `thread:${id}`);
  await turnTodoOn(first);
  assert.deepStrictEqual(shownIds(first), ["201", "202"]);
  await first.setVisibility("hidden");

  let reads = null;
  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    p.messages = data.messages;
    reads = gate(p.timeline);
    p.timeline = reads.respond;
  });
  // In the same task as the script: nothing has answered.
  assert.equal(page.screen(), "main");
  assert.equal(page.tab(), "discord", "the page did not reopen on the channel it was left on");
  assert.equal(page.el("thread-select").value, `thread:${id}`, "the thread was not reopened");
  assert.equal(page.el("thread-heading").hidden, false);
  assert.equal(wideCard(page).text, "first thread root", "the reopened thread has no card");
  assert.deepStrictEqual(shownIds(page), ["201", "202"], "the thread was not drawn from the device first");
  assert.equal(page.el("todo-filter").getAttribute("aria-pressed"), "true", "Hide read came back off");
  assert.deepStrictEqual(readKinds(page), [], "the reopen read before sign-in had answered");

  // Sign-in answers and the stream attaches: the first read is the thread on screen.
  await page.settle();
  assert.deepStrictEqual(readKinds(page), [`thread:${id}`], "the first read was not the thread on screen");
  assert.equal(wideCard(page).text, "first thread root", "sign-in lost the thread's card");
  reads.open();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["201", "202"]);
  assert.deepStrictEqual(readKinds(page), [`thread:${id}`], "the reopen read more than one page");
  assertCurrent(page, "the reopened thread still says it is saved");
  // Back goes where the thread was opened from: All.
  await page.el("thread-back").click();
  assert.equal(page.el("thread-select").value, "flat");
});

test("the reader's own choice of Main outlives the default across a reload", async () => {
  const first = await threadPage("main");
  assert.deepStrictEqual(savedUiState(first).channels, [{ channel: CHANNEL.id, channelView: "main", thread: null }],
    "picking Main was not recorded as this channel's choice");
  const page = reloadWith(first.storage, threadData().messages, (p) => {
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  assert.equal(page.el("thread-select").value, "main", "the default overrode the reader's choice");
  await reopenedChannel(page);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.deepStrictEqual(readKinds(page), ["main"], "the reopen read more than the view chosen");
});

test("a reload puts the reader back on the message they were reading, and on the newest when it is gone", async () => {
  const channel = tallChannel();
  const run = async (script) => {
    const first = newPage();
    await signIn(first);
    await showDiscord(first, channel);
    const area = first.el("scroll-area");
    area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
    const anchor = first.el("discord-log").children.find((li) => li.getBoundingClientRect().bottom > 0);
    const left = { id: anchor.getAttribute("data-id"), top: anchor.getBoundingClientRect().top };
    assert.ok(area.scrollTop > 0 && !atBottomOf(area), "the reader was not parked mid-channel");
    // The phone sends the app to the background, then kills it.
    await first.setVisibility("hidden");
    const page = newPage(first.storage, script, (p) => { p.messages = channel; });
    const row = () => page.el("discord-log").children.find((li) => li.getAttribute("data-id") === left.id);
    // Measured in the script's own task: drawn from the device, before sign-in has answered and so
    // before the reopen's read puts its line up over the rows.
    assert.equal(page.el("channel-loading").hidden, true, "the reopen read before sign-in answered");
    const before = row().getBoundingClientRect().top;
    await reopenedChannel(page);
    return { page, left, before, after: row().getBoundingClientRect().top, store: first.storage };
  };
  const { page, left, before, after, store } = await run(SCRIPT);
  assert.equal(before, left.top, "the reopen did not put the reader's message where it was");
  assert.equal(after, left.top, "the reopen's read moved the reader off their message");
  assert.equal(page.pageReads, 1, "the reopen read more than one page");

  // THE CONTROL: with the return deleted the reader lands on the newest message, and this fails.
  const control = await run(brokenScript(
    "  restoreScroll({ pinned: false, anchor: row, top: listPlaceTop(el(\"scroll-area\")) + place.offset });",
    "  void row;"
  ));
  assert.notEqual(control.before, control.left.top, "the test cannot tell a page that returns from one that does not");

  // The message is gone from the device — its snapshot given up to the quota — so there is nothing
  // to return to: the newest message, as a first visit.
  store.delete(MESSAGE_CACHE_KEY);
  const gone = reloadWith(store, channel.slice(-4));
  await reopenedChannel(gone);
  await gone.settle();
  assert.ok(atBottomOf(gone.el("scroll-area")), "with the reader's message gone, the reopen did not go to the newest");
});

test("a saved view that no longer holds falls back to the default rather than failing", async () => {
  // A thread the device no longer knows of — its snapshot is gone — opens the channel in All.
  const inThread = await threadPage();
  await pickThread(inThread, `thread:${inThread.threads[0].id}`);
  inThread.storage.delete(MESSAGE_CACHE_KEY);
  const lost = reloadWith(inThread.storage, threadData().messages, (p) => {
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  await reopenedChannel(lost);
  await lost.settle();
  assert.equal(lost.el("thread-select").value, "flat", "an unknown thread was reopened");
  assert.deepStrictEqual(readKinds(lost), ["flat"]);

  // A provider that no longer has threads opens it in Main, its whole channel.
  const threadless = await threadPage();
  await pickThread(threadless, `thread:${threadless.threads[0].id}`);
  threadless.storage.delete(MESSAGE_CACHE_KEY);
  const plain = reloadWith(threadless.storage, [message({ id: "320", content: "no threads here now" })]);
  await reopenedChannel(plain);
  await plain.settle();
  assert.equal(plain.el("thread-select").value, "main");
  assert.equal(plain.el("thread-heading").hidden, true, "a thread heading stood over a channel without threads");
  assert.deepStrictEqual(readKinds(plain), ["page"]);

  // A channel the server no longer lists: the picker's first channel, in its default view.
  const first = newPage();
  first.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  first.threadingSupported = true;
  first.threads = threadData().threads;
  await signIn(first);
  await showDiscord(first, threadData().messages);
  first.el("discord-channel").value = OTHER_CHANNEL.id;
  await first.el("discord-channel").dispatch("change");
  await first.settle();
  await pickThread(first, "main");
  assert.equal(savedUiState(first).channel, OTHER_CHANNEL.id);
  const unlisted = reloadWith(first.storage, threadData().messages, (p) => {
    p.channels = [{ ...CHANNEL }];
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  await reopenedChannel(unlisted);
  await unlisted.settle();
  assert.equal(unlisted.el("discord-channel").value, CHANNEL.id);
  assert.equal(unlisted.el("thread-select").value, "flat", "a choice made in a removed channel was applied to another");
  assert.match(channelReads(unlisted).at(-1), new RegExp(`/channels/${CHANNEL.id}/timeline\\?view=flat`));

  // A record that does not parse, or is another version's, is deleted: the page opens as a first visit does.
  for (const damaged of ["{\"v\":1,\"ident", JSON.stringify({ ...savedUiState(first), v: 99 })]) {
    const store = new Map(first.storage);
    store.set(UI_STATE_KEY, damaged);
    const page = reloadWith(store, threadData().messages, (p) => { p.threadingSupported = true; });
    assert.equal(page.tab(), "voice", "a damaged record reopened anything");
    assert.notEqual(store.get(UI_STATE_KEY), damaged, "a damaged record was kept");
    assert.equal(savedUiState(page).view, "voice", "the record left is not this page's own");
    await page.settle();
  }
});

test("Main drawn before threads were known gives way to All once the provider says it has them", async () => {
  // The device's shell predates the provider's threads, so the reopen draws Main; then
  // /client-config says otherwise before anything is read, and the view nobody chose becomes All —
  // once, with one read, of All.
  const first = newPage();
  await signIn(first);
  await showDiscord(first, [message({ id: "330", content: "from before threads" })]);
  assert.deepStrictEqual(savedUiState(first).channels, [], "a view nobody chose was recorded as a choice");
  const data = threadData();
  const page = reloadWith(first.storage, data.messages, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
  });
  assert.equal(page.el("thread-select").value, "main");
  await reopenedChannel(page);
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat", "learning of threads did not open All");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.deepStrictEqual(readKinds(page), ["flat"], "the Main the shell drew was read before All");
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat", "the view changed again");
  assert.deepStrictEqual(readKinds(page), ["flat", "flat"]);
});

test("Hide read on a channel without threads reopens on the to-do list, read once", async () => {
  const first = newPage();
  await signIn(first);
  await showDiscord(first, backlog());
  await turnTodoOn(first);
  assert.ok(first.todoReads > 0, "Hide read never read the to-do list");
  const page = reloadWith(first.storage, backlog());
  await reopenedChannel(page);
  assert.equal(page.el("todo-filter").getAttribute("aria-pressed"), "true");
  assert.equal(page.todoReads, 1, "the reopen did not read the to-do list once");
  assert.equal(page.pageReads, 0, "the reopen read the whole channel under Hide read");
});

test("a reload left on the call view reopens there, and reads no channel", async () => {
  const first = await threadPage();
  await first.el("view-switch").click();
  const page = reloadWith(first.storage, threadData().messages, (p) => { p.threadingSupported = true; });
  await page.settle();
  assert.equal(page.tab(), "voice");
  assert.deepStrictEqual(readKinds(page), [], "a channel nobody is looking at was read");
  assert.equal(page.el("thread-select").value, "flat", "the channel behind the call is not in its default");
});

test("a replay of what the reopen holds adds no read", async () => {
  // The burst may land after the reopen's read, or while it is out; a message the device holds
  // costs nothing either way. `#43 replay-burst-double-read`.
  const first = await threadPage();
  const data = threadData();
  const page = reloadWith(first.storage, data.messages, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
  });
  await reopenedChannel(page);
  await page.settle();
  assert.deepStrictEqual(readKinds(page), ["flat"]);
  await deliver(page, page.stream(), replayedFrames([data.messages[0], data.messages[3]]));
  await page.settle();
  assert.deepStrictEqual(readKinds(page), ["flat"], "a replay of held messages read the channel again");
});

/**
 * A warm threaded page left on the channel — in All, or in `view` when one is picked — reloaded over
 * the same storage, so the reload reopens ON the channel: the path a reload takes by default, and
 * not the one the `#43 replay-burst-double-read` counts above reload through. Sign-in has answered
 * when it returns.
 */
async function warmChannelReopen(arrange = null, view = null) {
  const data = threadData();
  const first = await threadPage(view);
  await first.setVisibility("hidden");
  const page = reloadWith(first.storage, [...data.messages], (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    if (arrange) arrange(p);
  });
  await reopenedChannel(page);
  return { page, data };
}

test("a reopen reads once its stream has attached, so an edit out of the tail costs no second read", async () => {
  // Only a read begun after the attach answers for the stream's replay tail. Begun before it — at
  // load, ahead of sign-in — an edit out of that tail waits on a read that cannot answer for it,
  // and makes another: two reads of the newest page on nearly every reopen.
  const { page, data } = await warmChannelReopen();
  await page.settle();
  assert.equal(page.streamOpens.length, 1, "the reopen never attached its live stream");
  assert.deepStrictEqual(readKinds(page), ["flat"], "the reopen did not read All once");
  const edited = { ...data.messages[0], content: "edited before the reload" };
  await deliver(page, page.stream(),
    sseUpdate("u1", edited, { replayed: true, from_tail: true }) +
    sseDelete("d1", CHANNEL.id, "203", { replayed: true, from_tail: true }));
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(readKinds(page), ["flat"], "an edit the reopen's read had already seen was read again");
  // The adapter's catch-up edit may be newer than that read, so it still gets one.
  await deliver(page, page.stream(), sseUpdate("u2", edited, { replayed: true }));
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(readKinds(page), ["flat", "flat"], "a catch-up edit lost its read");

  // As on a phone: the read is still on the wire when the burst lands.
  let reads = null;
  const { page: slow } = await warmChannelReopen((p) => {
    reads = gate(p.timeline);
    p.timeline = reads.respond;
  });
  assert.deepStrictEqual(readKinds(slow), ["flat"], "the reopen's read did not go out");
  await deliver(slow, slow.stream(), sseUpdate("u1", edited, { replayed: true, from_tail: true }));
  reads.open();
  await slow.settle();
  await slow.settle();
  await slow.settle();
  assert.deepStrictEqual(readKinds(slow), ["flat"], "a tail edit during the reopen's read queued another");
});

test("a reopen on Main reads nothing more for a reply out of the tail, in a thread Main does not show", async () => {
  const reply = message({ id: "250", content: "answered before the reload",
    thread: { ...threadData().messages[1].thread, is_root: false } });
  const { page, data } = await warmChannelReopen((p) => {
    p.messages.push(reply);
    p.messages[1] = withReplies(p.messages[1], 2);
  }, "main");
  await page.settle();
  assert.deepStrictEqual(readKinds(page), ["main"], "the reopen did not read the Main the reader chose");
  await deliver(page, page.stream(), replayedFrames([data.messages[0], reply]));
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(readKinds(page), ["main"], "a reply the reopen's read had already seen was read again");
  assert.equal(threadButton(rowShowing(page, "first thread root")).textContent, "2 replies");
  assert.ok(!shownIds(page).includes("250"), "a thread reply was dropped into Main");
});

test("a stream slow to attach holds a reopen's read back a few seconds, and a refused one not at all", async () => {
  const { page } = await warmChannelReopen((p) => {
    p.streamHeld = new Promise(() => {});
  });
  assert.deepStrictEqual(readKinds(page), [], "the reopen read before its stream attached");
  assert.equal(page.expireTimers(sourceConstant("LIVE_ATTACH_WAIT_MS")), 1, "nothing bounds the wait for the stream");
  await page.settle();
  assert.deepStrictEqual(readKinds(page), ["flat"], "a stream that never answered held the reopen's read back");

  const { page: refused } = await warmChannelReopen((p) => {
    p.streamStatus = 404;
  });
  assert.deepStrictEqual(readKinds(refused), ["flat"], "a refused stream held the reopen's read back");
  const { page: off } = await warmChannelReopen((p) => {
    p.liveDelivery = "off";
  });
  assert.equal(off.streamOpens.length, 0);
  assert.deepStrictEqual(readKinds(off), ["flat"], "a server that streams nothing held the reopen's read back");
});

test("a token refused at a reopen is reported as sign-in reports it, and nothing reads with it", async () => {
  // The saved rows go up at once, but nothing is read before /client-config has proved the token. A
  // read racing it answered the refusal over sign-in's own explanation, and armed a poll that went
  // on asking from behind the sign-in form.
  const first = newPage();
  await signIn(first);
  await showDiscord(first, [message({ id: "1301", content: "read with a token since revoked" })]);
  await first.setVisibility("hidden");
  const revoked = errorResponse(401, "unauthorized", "token revoked");
  const page = reloadPage(first.storage, (p) => {
    p.onlineConfig = p.clientConfig;
    p.clientConfig = revoked;
    p.channelPage = revoked;
  });
  assert.equal(page.tab(), "discord", "the saved rows did not reopen on the channel");
  await page.settle();
  await page.settle();
  assert.equal(page.screen(), "signin");
  assert.equal(page.el("error").textContent, "This server refused that token: HTTP 401 unauthorized: token revoked");
  assert.deepStrictEqual(page.requests, ["GET /api/v1/client-config"], "the channel was read with an unproven token");
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.deepStrictEqual(page.requests, ["GET /api/v1/client-config"], "the sign-in screen went on polling");

  // A token that works, and the channel left behind the form is read for it — once.
  page.clientConfig = page.onlineConfig;
  page.channelPage = async () => json(200, { channel: CHANNEL, messages: [message({ id: "1302", content: "read again" })] });
  await signIn(page);
  await page.settle();
  assert.equal(page.tab(), "discord");
  assert.equal(page.pageReads, 1, "the channel on screen was not read once for the token that works");
  assert.deepStrictEqual(shownIds(page), ["1302"]);
});

test("a page that opened offline reads once when the poll reaches the server, even for a provider grown threads", async () => {
  const first = newPage();
  await signIn(first);
  await showDiscord(first, [message({ id: "801", content: "kept offline" })]);
  await first.setVisibility("hidden");
  const data = threadData();
  const page = reloadWith(first.storage, data.messages, (p) => {
    p.onlineConfig = p.clientConfig;
    p.clientConfig = offline;
    p.channelPage = offline;
  });
  await reopenedChannel(page);
  await page.settle();
  assert.match(freshness(page).textContent, /^Offline · showing messages saved \d{2}:\d{2}$/);
  assert.deepStrictEqual(readKinds(page), ["page"], "the reopen did not try its one read");

  // Back online, under a provider that has threads now. Sign-in redraws the channel in All, and
  // the poll that asked for sign-in reads it: one read, not one for the redraw and one for the poll.
  page.threadingSupported = true;
  page.threads = data.threads;
  page.clientConfig = page.onlineConfig;
  assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "the failed reopen read did not arm the poll");
  await page.settle();
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat");
  assert.deepStrictEqual(readKinds(page), ["page", "flat"], "reaching the server read the channel twice");
  assertCurrent(page, "the recovered channel still says it is offline");
});

test("signing out forgets where the reader was, with the messages", async () => {
  const page = await threadPage();
  await pickThread(page, `thread:${page.threads[0].id}`);
  assert.ok(page.storage.has(UI_STATE_KEY));
  await page.el("forget-token").click();
  assert.equal(page.storage.has(UI_STATE_KEY), false, "the record outlived the credential that read its thread");
  const again = newPage(page.storage);
  assert.equal(again.screen(), "signin");
  await signIn(again);
  assert.equal(again.tab(), "voice", "a signed-out record reopened the channel");
});

// --- `#205 channel-view-memory`: each channel keeps the view the reader chose in it -------------
//
// The owner, 2026-10-04 16:18: "All should be the default but if changed the selection should be
// saved for that channel. Ideally it should survive restarts. Stored in local client state ideally
// but server side if needed." A choice used to last only until the channel changed. It is now kept
// per channel, in the record `#189 restore-ui-state` reopens on.

/** Two channels on providers with threads, each serving `threadData`: signed in, in the first, in All. */
async function twoThreadedChannels() {
  const page = newPage();
  const data = threadData();
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  return page;
}

/** A reload over the same storage and the same two channels. */
const reloadTwoChannels = (store, script = SCRIPT) => newPage(store, script, (p) => {
  p.messages = threadData().messages;
  p.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  p.threadingSupported = true;
  p.threads = threadData().threads;
});

test("a channel reopens in the view chosen in it, one nobody chose a view in opens in All, and each reads only that view", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const data = threadData();
    page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
    page.threadingSupported = true;
    page.threads = data.threads;
    await signIn(page);
    await showDiscord(page, data.messages);
    await pickThread(page, "main");
    const other = await changeChannelTo(page, OTHER_CHANNEL.id);
    const otherView = page.el("thread-select").value;
    const back = await changeChannelTo(page, CHANNEL.id);
    return { page, other, otherView, back };
  };
  const { page, other, otherView, back } = await run(SCRIPT);
  assert.equal(otherView, "flat", "a channel nobody chose a view in did not open in All");
  assert.deepStrictEqual(other, ["flat"], "the other channel was not read once, as All");
  assert.equal(page.el("thread-select").value, "main", "Main, chosen here, did not outlast a look at another channel");
  assert.deepStrictEqual(back, ["main"], "coming back did not read Main, once");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.deepStrictEqual(savedUiState(page).channels, [{ channel: CHANNEL.id, channelView: "main", thread: null }],
    "a channel nobody chose a view in was recorded as choosing one");

  // THE CONTROL: with the choice not kept, coming back opens in All, and this fails.
  const control = await run(brokenScript("  if (chosen) rememberChosenView(chosen);", "  if (chosen) void chosen;", 2));
  assert.equal(control.page.el("thread-select").value, "flat", "the test cannot tell a kept choice from none");

  // The Threads list is not a view a channel opens in: picked, it leaves the channel to its default.
  await page.el("channel-view-threads").click();
  await page.settle();
  assert.deepStrictEqual(savedUiState(page).channels, [], "the Threads list left Main standing as the channel's choice");

  // All, picked, is a choice like any other: it outlasts a default that is no longer All.
  await changeChannelTo(page, OTHER_CHANNEL.id);
  await pickThread(page, "main");
  await pickThread(page, "flat");
  await page.setVisibility("hidden");
  const mainByDefault = reloadTwoChannels(page.storage,
    brokenScript('  return threadingSupported ? "flat" : "main";', '  return "main";'));
  await reopenedChannel(mainByDefault);
  assert.equal(mainByDefault.el("discord-channel").value, OTHER_CHANNEL.id);
  assert.equal(mainByDefault.el("thread-select").value, "flat", "All, picked, gave way to the default");
  assert.deepStrictEqual(readKinds(mainByDefault), ["flat"]);
});

test("a thread chosen in one channel and Main in another both outlast a reload", async () => {
  const first = await twoThreadedChannels();
  const id = first.threads[0].id;
  // A finger touches the picker before it chooses: that reads the thread list, which names it.
  await first.el("thread-select").dispatch("pointerdown");
  await first.settle();
  await pickThread(first, `thread:${id}`);
  await changeChannelTo(first, OTHER_CHANNEL.id);
  await pickThread(first, "main");
  assert.deepStrictEqual(await changeChannelTo(first, CHANNEL.id), [`thread:${id}`], "coming back did not read the thread, once");
  assert.equal(first.el("thread-select").value, `thread:${id}`, "the thread chosen here was not reopened");
  assert.equal(wideCard(first).text, "first thread root");
  assert.deepStrictEqual(shownIds(first), ["201", "202"]);
  await changeChannelTo(first, OTHER_CHANNEL.id);
  await first.setVisibility("hidden");

  const page = reloadTwoChannels(first.storage);
  assert.equal(page.el("discord-channel").value, OTHER_CHANNEL.id);
  assert.equal(page.el("thread-select").value, "main", "the channel the app was left on did not reopen in its Main");
  await reopenedChannel(page);
  assert.deepStrictEqual(readKinds(page), ["main"], "the reopen read more than the view chosen");
  assert.deepStrictEqual(await changeChannelTo(page, CHANNEL.id), [`thread:${id}`],
    "the other channel did not reopen its thread with one read");
  assert.equal(page.el("thread-select").value, `thread:${id}`);
  assert.equal(wideCard(page).text, "first thread root", "the reopened thread has no card");
  assert.deepStrictEqual(shownIds(page), ["201", "202"]);
  // Back goes where the thread was opened from: All.
  await page.el("thread-back").click();
  await page.settle();
  assert.equal(page.el("thread-select").value, "flat");
});

test("a chosen thread falls back to All once the device no longer knows it, and to Main where threads are gone", async () => {
  const page = await twoThreadedChannels();
  const id = page.threads[0].id;
  await pickThread(page, `thread:${id}`);
  await changeChannelTo(page, OTHER_CHANNEL.id);
  // The device gives up its snapshot, and the first channel's thread with it.
  page.storage.delete(MESSAGE_CACHE_KEY);
  assert.deepStrictEqual(await changeChannelTo(page, CHANNEL.id), ["flat"], "the fallback did not read All, once");
  assert.equal(page.el("thread-select").value, "flat", "a thread the device no longer knows was reopened");
  assert.equal(page.el("thread-heading").hidden, true);
  assert.deepStrictEqual(savedUiState(page).channels, [], "the lost thread is still kept as the channel's choice");
  // All's read put the thread back in the store. The channel does not go back to it for that.
  await changeChannelTo(page, OTHER_CHANNEL.id);
  assert.deepStrictEqual(await changeChannelTo(page, CHANNEL.id), ["flat"]);
  assert.equal(page.el("thread-select").value, "flat", "a thread given up came back on a later visit");

  // A channel whose provider no longer has threads opens in Main, the whole channel, read once.
  const data = threadData();
  const providers = [{ ...CHANNEL, provider: "threaded" }, { ...OTHER_CHANNEL, provider: "elsewhere" }];
  const before = newPage();
  before.channels = providers;
  before.providers = [providerOf("threaded", true), providerOf("elsewhere", true)];
  before.threadingSupported = true;
  before.threads = data.threads;
  await signIn(before);
  await showDiscord(before, data.messages);
  await changeChannelTo(before, OTHER_CHANNEL.id);
  await pickThread(before, `thread:${id}`);
  await changeChannelTo(before, CHANNEL.id);
  const after = reloadWith(before.storage, data.messages, (p) => {
    p.channels = providers;
    p.providers = [providerOf("threaded", true), providerOf("elsewhere", false)];
    p.threadingSupported = true;
    p.threads = data.threads;
  });
  await reopenedChannel(after);
  assert.deepStrictEqual(await changeChannelTo(after, OTHER_CHANNEL.id), ["page"]);
  assert.equal(after.el("thread-select").value, "main", "a channel without threads did not open in Main");
  assert.equal(after.el("thread-heading").hidden, true, "a thread heading stood over a channel without threads");
});

test("the choices kept are bounded, and the channel least recently on screen gives its up first", async () => {
  const LIMIT = sourceConstant("UI_STATE_CHANNELS");
  const CHARS = sourceConstant("UI_STATE_CHARS");
  const rooms = Array.from({ length: LIMIT + 5 }, (_unused, i) =>
    ({ id: `33300000000000${String(i).padStart(5, "0")}`, label: `room ${i}`, writable: true }));
  const roomIds = rooms.map((room) => room.id);
  const first = await threadPage("main");
  const record = savedUiState(first);
  const reload = (choices) => {
    const store = new Map(first.storage);
    store.set(UI_STATE_KEY, JSON.stringify({ ...record, channels: choices }));
    return reloadWith(store, threadData().messages, (p) => {
      p.channels = [{ ...CHANNEL }, ...rooms];
      p.threadingSupported = true;
      p.threads = threadData().threads;
      holdCountsRead(p);
    });
  };

  // More choices than the bound, the most recent first, as a record from a larger bound would hold.
  const page = reload(rooms.map((room) => ({ channel: room.id, channelView: "main", thread: null })));
  await reopenedChannel(page);
  assert.deepStrictEqual(savedUiState(page).channels.map((entry) => entry.channel), roomIds.slice(0, LIMIT),
    "a record past the bound was kept past it, or not by recency");
  await pickThread(page, "main");
  assert.deepStrictEqual(savedUiState(page).channels.map((entry) => entry.channel), [CHANNEL.id, ...roomIds.slice(0, LIMIT - 1)],
    "a new choice did not push out the choice of the channel least recently on screen");
  assert.deepStrictEqual(await changeChannelTo(page, rooms[0].id), ["main"], "a kept choice did not open its channel");
  assert.equal(savedUiState(page).channels[0].channel, rooms[0].id, "a channel back on screen is not the last to go");

  // A few long thread roots cannot grow the record past its size: the least recent give way.
  const long = threadData().threads[0];
  const page2 = reload(rooms.slice(0, 30).map((room, i) => {
    const thread = `spaces/B/threads/${i}`;
    return {
      channel: room.id, channelView: "thread",
      thread: { id: thread, origin: "flat",
        summary: { ...long, id: thread, root: { ...long.root, content: "a long thread root. ".repeat(200) } } },
    };
  }));
  await reopenedChannel(page2);
  assert.ok(page2.storage.get(UI_STATE_KEY).length <= CHARS, "the record grew past its size");
  const kept = savedUiState(page2).channels.map((entry) => entry.channel);
  assert.ok(kept.length > 0 && kept.length < 30, `the size bound kept ${kept.length} of 30 choices`);
  assert.deepStrictEqual(kept, roomIds.slice(0, kept.length), "the size bound did not give up the least recent first");
});

test("a record saved before each channel kept its own view is carried forward, not discarded", async () => {
  // As the page before `#205 channel-view-memory` wrote it: one view, for the channel selected then,
  // and whether the reader chose it.
  const first = await threadPage();
  const id = first.threads[0].id;
  await first.el("thread-select").dispatch("pointerdown");
  await first.settle();
  await pickThread(first, `thread:${id}`);
  await first.setVisibility("hidden");
  const now = savedUiState(first);
  const v1 = {
    v: 1, identity: now.identity, view: "discord", todo: true, channel: CHANNEL.id,
    channelView: "thread", chosen: true, thread: { id, origin: "flat", summary: now.channels[0].thread.summary },
    place: null,
  };
  first.storage.set(UI_STATE_KEY, JSON.stringify(v1));
  const page = reloadWith(first.storage, threadData().messages, (p) => {
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  assert.equal(page.tab(), "discord", "a v1 record did not reopen the channel");
  assert.equal(page.el("thread-select").value, `thread:${id}`, "the thread a v1 record chose was not reopened");
  assert.equal(wideCard(page).text, "first thread root");
  assert.equal(page.el("todo-filter").getAttribute("aria-pressed"), "true", "a v1 record's Hide read was lost");
  await reopenedChannel(page);
  assert.deepStrictEqual(readKinds(page), [`thread:${id}`], "the reopen read more than the thread");
  const carried = savedUiState(page);
  assert.equal(carried.v, 2);
  assert.deepStrictEqual(carried.channels.map(({ channel, channelView, thread }) => [channel, channelView, thread.id, thread.origin]),
    [[CHANNEL.id, "thread", id, "flat"]], "the v1 record's choice did not become its channel's");

  // A place it kept is a place in the view it kept, and the reader comes back to their message.
  const channel = tallChannel();
  const tall = newPage();
  await signIn(tall);
  await showDiscord(tall, channel);
  const area = tall.el("scroll-area");
  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
  const anchor = tall.el("discord-log").children.find((li) => li.getBoundingClientRect().bottom > 0);
  const left = { id: anchor.getAttribute("data-id"), top: anchor.getBoundingClientRect().top };
  await tall.setVisibility("hidden");
  const kept = savedUiState(tall);
  assert.equal(kept.place.view, "main");
  const place = { ...kept.place };
  delete place.view;
  tall.storage.set(UI_STATE_KEY, JSON.stringify({
    v: 1, identity: kept.identity, view: "discord", todo: false, channel: CHANNEL.id,
    channelView: "main", chosen: false, thread: null, place,
  }));
  const back = newPage(tall.storage, SCRIPT, (p) => { p.messages = channel; });
  const row = back.el("discord-log").children.find((li) => li.getAttribute("data-id") === left.id);
  assert.equal(row.getBoundingClientRect().top, left.top, "a v1 record's place was not returned to");
  await reopenedChannel(back);
  assert.deepStrictEqual(savedUiState(back).channels, [], "a view the v1 record did not choose became a choice");

  // One that is not a sound v1 record is deleted, as a damaged record is.
  const store = new Map(first.storage);
  store.set(UI_STATE_KEY, JSON.stringify({ ...v1, chosen: "yes" }));
  const damaged = reloadWith(store, threadData().messages, (p) => { p.threadingSupported = true; });
  assert.equal(damaged.tab(), "voice", "a damaged v1 record reopened anything");
  assert.deepStrictEqual(savedUiState(damaged).channels, [], "a damaged v1 record left a choice behind");
  await damaged.settle();
});

test("a channel the server stops listing loses its choice with its saved rows", async () => {
  const first = await twoThreadedChannels();
  await changeChannelTo(first, OTHER_CHANNEL.id);
  await pickThread(first, `thread:${first.threads[0].id}`);
  await changeChannelTo(first, CHANNEL.id);
  await pickThread(first, "main");
  assert.deepStrictEqual(savedUiState(first).channels.map((entry) => entry.channel), [CHANNEL.id, OTHER_CHANNEL.id]);
  const page = reloadWith(first.storage, threadData().messages, (p) => {
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  await reopenedChannel(page);
  assert.deepStrictEqual(savedUiState(page).channels.map((entry) => entry.channel), [CHANNEL.id],
    "a channel no longer listed kept its choice, and the summary of the thread it chose");
});

test("signing out, or a token replaced underneath the page, forgets every channel's choice", async () => {
  const page = await twoThreadedChannels();
  await pickThread(page, "main");
  await changeChannelTo(page, OTHER_CHANNEL.id);
  await pickThread(page, `thread:${page.threads[0].id}`);
  assert.equal(savedUiState(page).channels.length, 2);
  await page.el("forget-token").click();
  assert.equal(page.storage.has(UI_STATE_KEY), false, "the choices outlived the credential that made them");
  await signIn(page);
  assert.equal(page.el("thread-select").value, "flat", "a signed-out choice reopened the channel on screen");
  await changeChannelTo(page, CHANNEL.id);
  assert.equal(page.el("thread-select").value, "flat", "a signed-out choice opened the other channel");

  // Another tab, or the main app, replaces the token without a sign-out here.
  const other = await twoThreadedChannels();
  await pickThread(other, "main");
  other.storage.set(TOKEN_STORAGE_KEY, "write-token-dddddddddddddddd");
  await changeChannelTo(other, OTHER_CHANNEL.id);
  await changeChannelTo(other, CHANNEL.id);
  assert.equal(other.el("thread-select").value, "flat", "a choice made under the last token opened a channel for the new one");
  assert.deepStrictEqual(savedUiState(other).channels, [], "the last token's choices were written under the new one's name");
});

// --- `#265 window-channel-memory`: each window of the page reopens on its own ------------------
//
// The owner, 2026-10-13, with two windows of the installed app on a desk, one left on each of two
// channels: "refresh will not respect those selections." The channel and the reopen record were
// kept only in the storage every window of the origin shares, so a reload came back on whatever the
// window that wrote last had shown. Each window now keeps its own as well, in `sessionStorage`, and
// the shared copy is what a brand-new window opens on. Here every `newPage` over the same map is
// another window of one device, and a reload of a window is one handed that window's own storage.

const ACTIVE_CHANNEL_KEY = "vibe-talk.voice.active-channel";
/** The channel each history read was for, in order. */
const channelsRead = (page) => channelReads(page).map((request) => request.split("/")[4]);

/**
 * A window on the two threaded channels over the device's `store`: a reload of a window when handed
 * its own `session`, and a brand-new window, with nothing of its own, when not.
 */
const windowOnTwoChannels = (store, session = null, script = SCRIPT) => newPage(store, script, (p) => {
  if (session) p.session = session;
  p.messages = threadData().messages;
  p.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  p.threadingSupported = true;
  p.threads = threadData().threads;
});

test("two windows left on two channels each reload on their own, and a new window opens on the one chosen last", async () => {
  const run = async (script) => {
    // The first window picks the second channel, and comes back to the first.
    const one = windowOnTwoChannels(new Map(), null, script);
    await signIn(one);
    await showDiscord(one, threadData().messages);
    await changeChannelTo(one, OTHER_CHANNEL.id);
    await changeChannelTo(one, CHANNEL.id);
    // A second window opens beside it, and is put on the second channel: the last choice made.
    const two = windowOnTwoChannels(one.storage, null, script);
    await reopenedChannel(two);
    const twoOpenedOn = two.el("discord-channel").value;
    await changeChannelTo(two, OTHER_CHANNEL.id);
    await one.setVisibility("hidden");
    await two.setVisibility("hidden");
    // Each window is reloaded, and then a third is opened.
    const oneAgain = windowOnTwoChannels(one.storage, one.session, script);
    await reopenedChannel(oneAgain);
    const twoAgain = windowOnTwoChannels(two.storage, two.session, script);
    await reopenedChannel(twoAgain);
    const three = windowOnTwoChannels(one.storage, null, script);
    await reopenedChannel(three);
    return { twoOpenedOn, oneAgain, twoAgain, three };
  };
  const { twoOpenedOn, oneAgain, twoAgain, three } = await run(SCRIPT);
  assert.equal(twoOpenedOn, CHANNEL.id, "a new window did not open on the channel chosen last");
  assert.equal(oneAgain.el("discord-channel").value, CHANNEL.id, "a reload took the channel another window chose");
  assert.deepStrictEqual(channelsRead(oneAgain), [CHANNEL.id], "the reload read a channel other than its own");
  assert.equal(twoAgain.el("discord-channel").value, OTHER_CHANNEL.id, "the second window's reload lost its channel");
  assert.deepStrictEqual(channelsRead(twoAgain), [OTHER_CHANNEL.id]);
  // A reload is not a choice: the first window coming back on its own channel left the last choice as it was.
  assert.equal(three.storage.get(ACTIVE_CHANNEL_KEY), OTHER_CHANNEL.id, "a reload took back the choice another window made");
  assert.equal(three.el("discord-channel").value, OTHER_CHANNEL.id, "a new window did not open on the channel chosen last");

  // THE CONTROL: with the window's own channel passed over, the first window reloads onto the
  // second's, as the owner's did, and this test fails.
  const control = await run(brokenScript("  const own = windowItem(ACTIVE_CHANNEL_KEY);\n  if (own) return own;\n", ""));
  assert.equal(control.oneAgain.el("discord-channel").value, OTHER_CHANNEL.id,
    "the test cannot tell a window's own channel from the one chosen last");
});

test("two windows on one channel each reopen in their own view, with the device's read mode and the other's choices behind their own", async () => {
  const run = async (script) => {
    const one = windowOnTwoChannels(new Map(), null, script);
    await signIn(one);
    await showDiscord(one, threadData().messages);
    await pickThread(one, "main");
    // A second window opens in Main, the channel's choice; and then goes into a thread there, picks
    // Main in the other channel, comes back, and turns Hide read on.
    const two = windowOnTwoChannels(one.storage, null, script);
    await reopenedChannel(two);
    const twoOpenedIn = two.el("thread-select").value;
    const id = two.threads[0].id;
    // A finger touches the picker before it chooses: that reads the thread list, which names it.
    await two.el("thread-select").dispatch("pointerdown");
    await two.settle();
    await pickThread(two, `thread:${id}`);
    await changeChannelTo(two, OTHER_CHANNEL.id);
    await pickThread(two, "main");
    await changeChannelTo(two, CHANNEL.id);
    await turnTodoOn(two);
    await one.setVisibility("hidden");
    await two.setVisibility("hidden");
    const oneAgain = windowOnTwoChannels(one.storage, one.session, script);
    await reopenedChannel(oneAgain);
    const twoAgain = windowOnTwoChannels(two.storage, two.session, script);
    await reopenedChannel(twoAgain);
    return { id, twoOpenedIn, oneAgain, twoAgain };
  };
  const { id, twoOpenedIn, oneAgain, twoAgain } = await run(SCRIPT);
  assert.equal(twoOpenedIn, "main", "a new window did not open in the view chosen in the channel");
  assert.equal(oneAgain.el("discord-channel").value, CHANNEL.id);
  assert.equal(oneAgain.el("thread-select").value, "main", "a reload opened in the view another window had open");
  assert.equal(twoAgain.el("thread-select").value, `thread:${id}`, "the second window's reload lost its thread");
  assert.equal(wideCard(twoAgain).text, "first thread root");
  // The read mode is the device's, not a place: turned on in one window, a reload of the other has it.
  for (const page of [oneAgain, twoAgain]) {
    assert.equal(page.el("todo-filter").getAttribute("aria-pressed"), "true", "the read mode was not the device's");
  }
  // A view chosen in another window, in a channel this one chose none in, is honoured here too.
  await changeChannelTo(oneAgain, OTHER_CHANNEL.id);
  assert.equal(oneAgain.el("thread-select").value, "main", "the view chosen in another window was not honoured");

  // THE CONTROL: with the window's own record passed over, the first window reopens in the second's thread.
  const control = await run(brokenScript("  const own = readUiRecord(windowStore);", "  const own = null;"));
  assert.equal(control.oneAgain.el("thread-select").value, `thread:${control.id}`,
    "the test cannot tell a window's own view from the device's");
});

test("`#281 view-keys` x `#265 window-channel-memory`: each window reopens in its own one of the four views, and a new window in the one chosen last", async () => {
  /** A window on a wide desk over the device's `store`: a reload when handed its own `session`. */
  const deskWindow = (store, session = null) => newPage(store, SCRIPT, (p) => {
    if (session) p.session = session;
    p.wideDesk = true;
    p.messages = threadData().messages;
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  const view = (page) => page.el("thread-select").value;
  const one = deskWindow(new Map());
  await signIn(one);
  await showDiscord(one, threadData().messages);
  await press(one, one.el("scroll-area"), "3", { ctrlKey: true });
  await one.settle();
  assert.equal(view(one), "threads");
  // A second window opens in the layout chosen last, and is put in two columns.
  const two = deskWindow(one.storage);
  await reopenedChannel(two);
  await two.settle();
  assert.equal(view(two), "threads", "a new window did not open in the view chosen last");
  await press(two, two.el("scroll-area"), "4", { metaKey: true });
  await two.settle();
  assert.equal(view(two), "columns");
  await one.setVisibility("hidden");
  await two.setVisibility("hidden");
  // Each window reloads in its own view; a third, new, in the one chosen last.
  const oneAgain = deskWindow(one.storage, one.session);
  await reopenedChannel(oneAgain);
  await oneAgain.settle();
  assert.equal(view(oneAgain), "threads", "a reload took the view another window chose");
  assert.equal(oneAgain.el("screen-main").getAttribute("data-columns"), "list");
  const twoAgain = deskWindow(two.storage, two.session);
  await reopenedChannel(twoAgain);
  await twoAgain.settle();
  assert.equal(view(twoAgain), "columns", "the second window's reload lost two columns");
  const three = deskWindow(one.storage);
  await reopenedChannel(three);
  await three.settle();
  assert.equal(view(three), "columns", "a new window did not open in the view chosen last");
  // And one column: Ctrl+2 in the first window is All, after its reload too.
  await press(oneAgain, oneAgain.el("side-scroll"), "2", { ctrlKey: true });
  await oneAgain.settle();
  assert.equal(view(oneAgain), "flat");
  await oneAgain.setVisibility("hidden");
  const oneLast = deskWindow(oneAgain.storage, oneAgain.session);
  await reopenedChannel(oneLast);
  await oneLast.settle();
  assert.equal(view(oneLast), "flat", "a reload of a window left in All did not come back to All");
  assert.equal(oneLast.el("screen-main").hasAttribute("data-columns"), false);
});

test("a window's channel the server stops listing falls back to its first, as before; signing out forgets the window's record", async () => {
  const one = windowOnTwoChannels(new Map());
  await signIn(one);
  await showDiscord(one, threadData().messages);
  await changeChannelTo(one, OTHER_CHANNEL.id);
  await pickThread(one, "main");
  await one.setVisibility("hidden");
  assert.equal(one.session.get(ACTIVE_CHANNEL_KEY), OTHER_CHANNEL.id);
  assert.equal(JSON.parse(one.session.get(UI_STATE_KEY)).channel, OTHER_CHANNEL.id);
  const onlyFirst = (session) => newPage(one.storage, SCRIPT, (p) => {
    if (session) p.session = session;
    p.messages = threadData().messages;
    p.channels = [{ ...CHANNEL }];
    p.threadingSupported = true;
    p.threads = threadData().threads;
  });
  // A reload of the window, and a brand-new window whose last choice is that channel: both on the first.
  const again = onlyFirst(one.session);
  await again.settle();
  assert.equal(again.el("discord-channel").value, CHANNEL.id, "a reload kept a channel the server no longer lists");
  assert.equal(again.session.get(ACTIVE_CHANNEL_KEY), CHANNEL.id, "the window still holds the channel that went");
  const fresh = onlyFirst(null);
  await fresh.settle();
  assert.equal(fresh.el("discord-channel").value, CHANNEL.id, "a new window opened on a channel no longer listed");

  // Signing out takes the record off the device and out of the window.
  await again.el("forget-token").click();
  assert.equal(again.storage.has(UI_STATE_KEY), false, "the record outlived the credential on the device");
  assert.equal(again.session.has(UI_STATE_KEY), false, "the record outlived the credential in the window");
});

/** The layout of the main screen: "list", "two", or null for one column. */
const layoutOf = (page) => page.el("screen-main").getAttribute("data-columns");

/**
 * `threadPage`'s channel — two threads, opened in All — on a page that keeps a session history, and,
 * with `closeWatcher`, close watchers; `history` says what the tab held before the page loaded, and
 * whether the window has the Navigation API (`enableHistory`).
 */
async function backPage({ script = SCRIPT, closeWatcher = false, history = {}, arrange = null } = {}) {
  const page = newPage(new Map(), script, (p) => {
    p.enableHistory(history);
    if (closeWatcher) p.enableCloseWatcher();
    if (arrange) arrange(p);
  });
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  await signIn(page);
  await showDiscord(page, data.messages);
  await page.settle();
  return page;
}

/** Open the channel's first thread from the picker, as a finger does. */
const openFirstThread = (page) => pickThread(page, `thread:${page.threads[0].id}`);

/** A thread opened, and one gesture after it, on `script`: what each came to, for the walk and its controls. */
async function threadThenGesture(script = SCRIPT) {
  const page = await backPage({ script });
  const before = { length: page.history.length, facts: backFacts(page) };
  await openFirstThread(page);
  const opened = { grew: page.history.length - before.length, facts: backFacts(page),
    thread: !page.el("thread-heading").hidden };
  await page.browserBack();
  const after = { thread: !page.el("thread-heading").hidden, picker: page.el("thread-select").value,
    facts: backFacts(page), left: page.left };
  return { page, before, opened, after };
}

test("`#304 back-gesture-in-app`: a thread opened is one entry at the page's own address, the gesture closes it as Esc does, the keys on the list, and at the top the gesture leaves the app", async () => {
  const { page, before, opened, after } = await threadThenGesture();
  // At the top the page's entry is its own base, and there is nothing of the page's above it.
  assert.deepStrictEqual(before.facts, { depth: 0, tag: "top" }, "the page did not make the entry it opened on its base");
  assert.equal(opened.thread, true, "fixture: the picker did not open the thread");
  assert.equal(opened.grew, 1, "opening a thread did not push exactly one entry");
  assert.deepStrictEqual(opened.facts, { depth: 1, tag: "thread" });
  // `#462 webapp-deep-links`: an entry no longer leaves the address alone — the query says
  // where the reader is — but it is replaced into the same entry with its state untouched
  // (the facts above), never pushed: the thread is still exactly one entry.
  assert.ok(page.history.addresses.length > 0 &&
    page.history.addresses.every((address) => address.startsWith("/?c=")),
    `an entry wrote an address that is not the place's own query: ${page.history.addresses}`);
  assert.ok(page.history.addresses.some((address) => /[?&]v=thread&/.test(address)),
    "no address named the thread while it was open");
  // The gesture: the thread closes, back to the view it was opened from, and the app is still here.
  assert.equal(after.left, false, "the gesture in a thread left the app");
  assert.equal(after.thread, false, "the gesture in a thread did not close it");
  assert.equal(after.picker, "flat", "the gesture did not go back to All, where the thread was opened from");
  assert.deepStrictEqual(after.facts, { depth: 0, tag: "top" });
  // The same focus as Esc's: the list on screen holds the keys.
  assert.ok(focused(page) && nodeWithinFixture(page.el("scroll-area"), focused(page)),
    `after the gesture the focus is on ${focusName(page)}, not on the list`);
  assert.deepStrictEqual(page.history.asked, [], "the gesture's close went back through another entry");
  // At the top, the gesture is the browser's: it leaves the app, and the page changed nothing on the way.
  await page.browserBack();
  assert.equal(page.left, true, "the gesture at the top did not leave the app");
  assert.equal(page.el("thread-select").value, "flat");
  // Help says so, where the channel is explained.
  assert.match(HTML_CODE, /<strong>back gesture<\/strong>, and the browser's Back, close what is open\s+over the channel/,
    "Help's entry on the channel does not say what the back gesture does");

  // NEGATIVE CONTROLS. With no popstate listener the gesture lands on the page's base and leaves the
  // thread open; with no entry pushed at all, it leaves the app from inside the thread — the owner's report.
  const deaf = await threadThenGesture(brokenScript('window.addEventListener("popstate", onBackPop);', ""));
  assert.equal(deaf.after.thread, true, "the control without a popstate listener closed the thread anyway");
  const flat = await threadThenGesture(brokenScript('history.pushState(backEntry(at + 1, levels[at]), "");', ""));
  assert.equal(flat.opened.grew, 0, "the control that pushes nothing pushed an entry");
  assert.equal(flat.after.left, true, "the control that pushes nothing did not leave the app from the thread");
  assert.equal(flat.after.thread, true);
});

/** The thread list, a thread opened from its first card, and the gesture three times, on `script`. */
async function listThreadWalk(script = SCRIPT) {
  const page = await backPage({ script });
  const seen = [];
  const note = (what) => seen.push({ what, layout: layoutOf(page), thread: !page.el("thread-heading").hidden,
    picker: page.el("thread-select").value, facts: backFacts(page), left: page.left });
  await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
  await page.settle();
  note("Ctrl+3");
  const card = page.el("side-cards").children.find((li) => li.hasAttribute("data-card"));
  assert.ok(card, "fixture: the thread list drew no card");
  await openerOf(card).click();
  await page.settle();
  note("the card's replies");
  for (let i = 1; i <= 3; i += 1) {
    await page.browserBack();
    note(`gesture ${i}`);
  }
  return { page, seen, card };
}

test("`#304 back-gesture-in-app`: NESTED — the thread list, a thread opened from it, then the gesture: the list, then the channel, then out of the app", async () => {
  const { seen, card } = await listThreadWalk();
  const [list, thread, first, second, third] = seen;
  assert.equal(list.layout, "list", "fixture: Ctrl+3 did not show the thread list");
  assert.deepStrictEqual(list.facts, { depth: 1, tag: "list" }, "the thread list is not one entry above the channel");
  assert.equal(thread.layout, null);
  assert.equal(thread.thread, true, "fixture: the card did not open its thread");
  assert.equal(thread.picker, `thread:${card.getAttribute("data-id")}`);
  assert.deepStrictEqual(thread.facts, { depth: 2, tag: "thread" }, "the thread opened from the list is not an entry above it");
  assert.equal(first.left, false);
  assert.equal(first.layout, "list", "the first gesture did not go back to the thread list");
  assert.equal(first.thread, false);
  assert.deepStrictEqual(first.facts, { depth: 1, tag: "list" });
  assert.equal(second.left, false);
  assert.equal(second.layout, null, "the second gesture did not leave the thread list");
  assert.equal(second.picker, "flat", "the second gesture did not go back to All, where the list was chosen from");
  assert.deepStrictEqual(second.facts, { depth: 0, tag: "top" });
  assert.equal(third.left, true, "the third gesture, at the top, did not leave the app");

  // NEGATIVE CONTROL: a popstate that closes everything rather than down to its entry's depth skips the list.
  const greedy = await listThreadWalk(brokenScript("closeBackLevelsTo(closeTo);", "closeBackLevelsTo(0);"));
  assert.notEqual(greedy.seen[2].layout, "list", "the control that closes everything still came back to the list");
});

/**
 * Open something, close it by its own control, then land what the page asked for and take one gesture:
 * the walk for every on-screen way out, on `script`.
 */
async function closeThenGesture(open, close, script = SCRIPT) {
  const page = await backPage({ script });
  await open(page);
  await page.settle();
  const opened = backFacts(page);
  await close(page);
  await page.settle();
  const closed = { screen: page.screen(), thread: !page.el("thread-heading").hidden, asked: [...page.history.asked] };
  await page.traverse();
  const landed = backFacts(page);
  await page.browserBack();
  return { page, opened, closed, landed, left: page.left };
}

const ON_SCREEN_WAYS_OUT = [
  ["the thread's ← Back", openFirstThread, (page) => page.el("thread-back").click(), "thread"],
  ["Esc in the thread", openFirstThread, (page) => press(page, page.el("scroll-area"), "Escape"), "thread"],
  ["Settings' Back", (page) => page.el("open-settings").click(), (page) => page.el("close-settings").click(), "settings"],
  ["Esc in Settings", (page) => page.el("open-settings").click(),
    (page) => press(page, page.el("screen-settings"), "Escape"), "settings"],
  ["the reply screen's Cancel", (page) => replyButton(page.el("discord-log").children[0]).click(),
    (page) => page.el("reply-cancel").click(), "reply"],
];

test("`#304 back-gesture-in-app`: THE ON-SCREEN WAYS OUT close at once and go back through their entry, so the next gesture leaves the app rather than landing on a stale one", async () => {
  for (const [what, open, close, tag] of ON_SCREEN_WAYS_OUT) {
    const { page, opened, closed, landed, left } = await closeThenGesture(open, close);
    assert.deepStrictEqual(opened, { depth: 1, tag }, `${what}: what it closes was not one entry`);
    assert.equal(closed.screen, "main", `${what} did not close at once`);
    assert.equal(closed.thread, false, `${what} left a thread open`);
    assert.deepStrictEqual(closed.asked, [-1], `${what} did not go back through its entry, once`);
    assert.deepStrictEqual(landed, { depth: 0, tag: "top" }, `${what}: the page's own going-back did not land on its base`);
    assert.equal(page.screen(), "main", `${what}: the page's own going-back closed something more`);
    assert.equal(left, true, `${what}: the gesture after it did not leave the app — it spent a press on a stale entry`);
  }
  // NEGATIVE CONTROL: a page that never goes back through an entry leaves the stale one under the next gesture.
  const stale = await closeThenGesture(openFirstThread, (page) => page.el("thread-back").click(),
    brokenScript("if (!backTraversalStuck) goBackThrough(history, depth - wanted);", ""));
  assert.deepStrictEqual(stale.closed.asked, [], "the control went back through an entry");
  assert.equal(stale.left, false, "the control's gesture left the app despite the stale entry");
});

test("`#304 back-gesture-in-app`: the page's own going-back is never taken for the reader's, even when they opened something before it landed", async () => {
  const page = await backPage();
  await openFirstThread(page);
  await press(page, page.el("scroll-area"), "Escape");
  assert.deepStrictEqual(page.history.asked, [-1]);
  // The thread opened again before the browser landed the going-back: no entry yet, as one would be
  // pushed onto the entry being left.
  await openFirstThread(page);
  assert.equal(page.el("thread-heading").hidden, false);
  const pushes = page.history.pushes;
  await page.traverse();
  assert.equal(page.el("thread-heading").hidden, false, "the page's own going-back closed the thread opened after it");
  assert.equal(page.history.pushes, pushes + 1, "the thread opened meanwhile got no entry once the going-back landed");
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "thread" });
  await page.browserBack();
  assert.equal(page.el("thread-heading").hidden, true, "the gesture did not close the thread opened meanwhile");
  assert.equal(page.left, false);
});

/** A tab whose entries an earlier load of the page made, three deep, and the page reloaded on the top one. */
const EARLIER_LOAD = "an-earlier-load";
const earlier = (depth, tag) => ({ vibeTalkBack: { session: EARLIER_LOAD, depth, tag } });
const reloadedTab = (navigation, leading = true) =>
  ({ state: earlier(2, "thread"), below: [earlier(0, "top"), earlier(1, "list")], navigation, leading });

/**
 * The page reloaded on `reloadedTab`, reopening nothing of it (this fixture keeps no view to reopen), what
 * the earlier load's entries are gone back through as landed, then a thread opened and closed by the
 * gesture: where each step leaves it.
 */
async function reloadedThenThread(navigation, { leading = true, script = SCRIPT } = {}) {
  const page = await backPage({ script, history: reloadedTab(navigation, leading) });
  const loaded = { facts: backFacts(page), session: page.history.state.vibeTalkBack.session,
    asked: [...page.history.asked], picker: page.el("thread-select").value };
  await page.traverse();
  const landed = { facts: backFacts(page), session: page.history.state.vibeTalkBack.session, index: page.history.index,
    picker: page.el("thread-select").value, thread: !page.el("thread-heading").hidden };
  await openFirstThread(page);
  const opened = { facts: backFacts(page), length: page.history.length };
  await page.browserBack();
  return { page, loaded, landed, opened, closed: page.el("thread-heading").hidden };
}

test("`#304 back-gesture-in-app`: A RELOAD that reopens nothing goes back through the earlier load's entries at once, to that load's base, which is its own: no stale entry is left under it, with the Navigation API or without, and with the app the tab's first entry the Back at the top is the system's", async () => {
  for (const [navigation, leading] of [[true, true], [false, true], [true, false], [false, false]]) {
    const how = `${navigation ? "with" : "without"} the Navigation API, ${leading ? "after a page elsewhere" : "the app first"}`;
    const { page, loaded, landed, opened, closed } = await reloadedThenThread(navigation, { leading });
    // The entry reloaded on is this load's own, at the depth the earlier load gave it, and the two under
    // it stand for nothing now open: gone back through, both, in one going-back.
    assert.deepStrictEqual(loaded.facts, { depth: 2, tag: "thread" }, `${how}: the entry reloaded on is not this load's at its depth`);
    assert.notEqual(loaded.session, EARLIER_LOAD, `${how}: the page kept the earlier load's name`);
    assert.deepStrictEqual(loaded.asked, [-2], `${how}: the page did not go back through the earlier load's two entries at once`);
    assert.equal(loaded.picker, "flat", `${how}: the page opened something for an earlier load's entry`);
    assert.deepStrictEqual(landed.facts, { depth: 0, tag: "top" }, `${how}: the earlier load's base is not this load's`);
    assert.notEqual(landed.session, EARLIER_LOAD, `${how}: the base kept the earlier load's name`);
    assert.equal(landed.index, page.history.first, `${how}: the going-back did not land on the earlier load's base`);
    assert.equal(landed.picker, "flat", `${how}: landing on the base changed the view`);
    assert.equal(landed.thread, false);
    // A thread from there: one entry over the base, the stale ones ahead cut off.
    assert.deepStrictEqual(opened.facts, { depth: 1, tag: "thread" });
    assert.equal(opened.length, page.history.first + 2, `${how}: the earlier load's entries were not cut off by the next one pushed`);
    assert.equal(closed, true);
    assert.equal(page.left, false);
    // At the top, nothing of the app's own is left under the base: one press leaves it.
    assert.equal(page.history.index, page.history.first, `${how}: there is an entry of the app's under its base`);
    await page.browserBack();
    assert.equal(page.left, true, `${how}: the gesture at the top did not leave the app`);
    assert.deepStrictEqual(page.history.refused, [], `${how}: the page asked to go past the end of the history, or to reload`);
  }

  // AN ENTRY CLAIMING MORE than the browser has under it: the page goes back no further than the
  // Navigation API says there are entries of its own, so never past the tab's first.
  const claiming = (script = SCRIPT) => backPage({ script,
    history: { state: earlier(5, "thread"), below: [earlier(0, "top"), earlier(1, "list")], leading: false } });
  const over = await claiming();
  assert.deepStrictEqual(backFacts(over), { depth: 2, tag: "thread" }, "the page took a depth deeper than the entries under it");
  await over.traverse();
  assert.deepStrictEqual(over.history.refused, []);
  assert.equal(over.history.index, 0);
  assert.deepStrictEqual(backFacts(over), { depth: 0, tag: "top" });

  // A FORWARD onto an earlier load's entry ahead is not a Back: nothing closes or opens, and the page
  // goes back through it to its own, so the next Back is not spent on it.
  const forward = async (script = SCRIPT) => {
    const ahead = await backPage({ script, history: { above: [earlier(1, "thread"), earlier(2, "list")] } });
    ahead.history.go(1);
    await ahead.traverse();
    return ahead;
  };
  const ahead = await forward();
  assert.equal(ahead.left, false, "a Forward onto an earlier load's entry left the app");
  assert.equal(ahead.el("thread-heading").hidden, true);
  assert.equal(ahead.el("thread-select").value, "flat");
  assert.equal(ahead.history.index, 1, "the page did not go back through the entry a Forward landed on");
  assert.deepStrictEqual(backFacts(ahead), { depth: 0, tag: "top" });
  await ahead.browserBack();
  assert.equal(ahead.left, true, "after a Forward onto a stale entry, the gesture at the top did not leave the app");

  // NEGATIVE CONTROLS. A page that makes the entry it reloaded on its base at depth nought, as this
  // change first did, leaves the earlier load's entries under it, and the gesture at the top lands on
  // one; one that never clamps asks to go past the tab's first entry, which the browser ignores; and one
  // that takes a Forward for a Back makes the stale entry its base, over the base it had.
  const based = await reloadedThenThread(true, {
    script: brokenScript("const depth = earlier ? Math.min(earlier.depth, index === null ? BACK_DEPTH_MAX : index, BACK_DEPTH_MAX) : 0;",
      "const depth = 0;") });
  assert.deepStrictEqual(based.loaded.asked, [], "the control that reloads at depth nought went back through entries");
  await based.page.browserBack();
  assert.equal(based.page.left, false, "the control's gesture at the top did not land on a stale entry");
  const unclamped = await claiming(brokenScript("Math.min(earlier.depth, index === null ? BACK_DEPTH_MAX : index, BACK_DEPTH_MAX)",
    "Math.min(earlier.depth, BACK_DEPTH_MAX)"));
  await unclamped.traverse();
  assert.deepStrictEqual(unclamped.history.refused, [-5], "the control that never clamps did not ask past the first entry");
  const blind = await forward(brokenScript("} else if (moved !== null && moved > 0) {", "} else if (false) {"));
  await blind.browserBack();
  assert.equal(blind.left, false, "the control that takes a Forward for a Back left the app in one press");
});

/**
 * The page reloaded from `first`: its storage, and the tab's history as it left it — the entries under
 * the one it was on, that one, what was ahead — with a page elsewhere before the app or not, as `first`
 * had. The reader has not touched the page since, as when a phone reloads an app it put away
 * (`activation`, Chromium's `navigator.userActivation`, unless the test says otherwise).
 */
function reloadedFrom(first, data, { script = SCRIPT, activation = { hasBeenActive: false } } = {}) {
  const { entries, index, first: start } = first.history;
  return newPage(first.storage, script, (p) => {
    p.enableHistory({ state: entries[index].state, below: entries.slice(start, index).map((entry) => entry.state),
      above: entries.slice(index + 1).map((entry) => entry.state), leading: start === 1 });
    if (activation) p.setUserActivation(activation);
    p.threadingSupported = true;
    p.threads = data.threads;
    p.messages = data.messages;
  });
}

/**
 * A thread opened from the thread list (or, `single`, from the picker), the page reloaded on it, and
 * the gesture until it leaves: what the reload reopened, and what each gesture came to.
 */
async function reloadWalk({ leading = true, single = false, script = SCRIPT, activation } = {}) {
  const first = await backPage({ history: { leading } });
  if (single) {
    await openFirstThread(first);
  } else {
    await press(first, first.el("scroll-area"), "3", { ctrlKey: true });
    await first.settle();
    await openerOf(first.el("side-cards").children.find((li) => li.hasAttribute("data-card"))).click();
    await first.settle();
  }
  const before = { facts: backFacts(first), length: first.history.length, session: first.history.state.vibeTalkBack.session };
  await first.setVisibility("hidden");
  const page = reloadedFrom(first, threadData(), { script, ...(activation === undefined ? {} : { activation }) });
  await page.settle();
  await page.settle();
  const note = () => ({ layout: layoutOf(page), thread: !page.el("thread-heading").hidden, picker: page.el("thread-select").value,
    facts: page.left ? null : backFacts(page), index: page.history.index, left: page.left });
  const reopened = { ...note(), session: page.history.state.vibeTalkBack.session, pushes: page.history.pushes,
    asked: [...page.history.asked], length: page.history.length };
  const gestures = [];
  for (let i = 0; i < 5 && !page.left; i += 1) {
    await page.browserBack();
    gestures.push(note());
  }
  return { page, before, reopened, gestures };
}

test("`#304 back-gesture-in-app`: A RELOAD in a thread opened from the thread list reopens both on the earlier load's two entries, nothing pushed: the first Back, before any touch, closes the thread alone, the next the list, and the next leaves — and with the app the tab's first entry, it is the system's", async () => {
  for (const leading of [true, false]) {
    const how = leading ? "after a page elsewhere" : "the app first";
    const { page, before, reopened, gestures } = await reloadWalk({ leading });
    assert.deepStrictEqual(before.facts, { depth: 2, tag: "thread" }, "fixture: the thread opened from the list is not two deep");
    assert.equal(reopened.thread, true, `${how}: fixture: the reload did not reopen the thread (#189)`);
    assert.equal(reopened.layout, null);
    assert.deepStrictEqual(reopened.facts, { depth: 2, tag: "thread" },
      `${how}: the reopened thread does not stand on the earlier load's entries, two deep`);
    assert.notEqual(reopened.session, before.session, `${how}: the reloaded page kept the earlier load's name`);
    assert.equal(reopened.pushes, 0, `${how}: the reload pushed entries where the earlier load's stand`);
    assert.deepStrictEqual(reopened.asked, [], `${how}: the reload went back through entries it stands on`);
    assert.equal(reopened.length, before.length, `${how}: the reload changed how many entries the tab has`);
    const [first, second, third] = gestures;
    assert.equal(first.left, false, `${how}: the first Back after the reload left the app`);
    assert.equal(first.thread, false, `${how}: the first Back did not close the reopened thread`);
    assert.equal(first.layout, "list", `${how}: the first Back after the reload closed more than the thread: ${JSON.stringify(first)}`);
    assert.deepStrictEqual(first.facts, { depth: 1, tag: "list" });
    assert.equal(second.left, false);
    assert.equal(second.layout, null, `${how}: the second Back did not leave the thread list`);
    assert.equal(second.picker, "flat");
    assert.deepStrictEqual(second.facts, { depth: 0, tag: "top" });
    assert.equal(second.index, page.history.first, `${how}: an entry of the app's is left under its base, a press spent on nothing`);
    assert.equal(third.left, true, `${how}: the third Back did not leave the app`);
    assert.equal(gestures.length, 3);
    assert.deepStrictEqual(page.history.refused, []);
  }
  // One thread, the app first: one Back closes it, and the next is the system's.
  const one = await reloadWalk({ leading: false, single: true });
  assert.deepStrictEqual(one.reopened.facts, { depth: 1, tag: "thread" });
  assert.equal(one.gestures[0].thread, false, "the Back after a reload in one thread did not close it");
  assert.equal(one.gestures[0].index, 0, "an entry of the app's is left under its base after a reload in one thread");
  assert.equal(one.gestures[1].left, true);

  // TWO ENTRIES AT ONCE, from the browser's long-press history list, onto the earlier load's base: both
  // things close, as that entry is taken for this load's at the depth it says.
  const jumpedFrom = async (script = SCRIPT) => {
    const first = await backPage();
    await press(first, first.el("scroll-area"), "3", { ctrlKey: true });
    await first.settle();
    await openerOf(first.el("side-cards").children.find((li) => li.hasAttribute("data-card"))).click();
    await first.settle();
    await first.setVisibility("hidden");
    const page = reloadedFrom(first, threadData(), { script });
    await page.settle();
    await page.settle();
    page.history.go(-2);
    await page.traverse();
    return { page, layout: layoutOf(page), thread: !page.el("thread-heading").hidden, picker: page.el("thread-select").value };
  };
  const jumped = await jumpedFrom();
  assert.equal(jumped.thread, false, "a jump of two entries did not close the reopened thread");
  assert.equal(jumped.layout, null, "a jump of two entries onto the earlier load's base left the thread list open");
  assert.equal(jumped.picker, "flat");
  assert.deepStrictEqual(backFacts(jumped.page), { depth: 0, tag: "top" });
  await jumped.page.browserBack();
  assert.equal(jumped.page.left, true, "after the jump, the gesture at the top did not leave the app");

  // MORE REOPENED than the earlier load has entries for — that load's own reopen, never touched — the
  // entry it is short waits for the reader's touch, and a Back before it closes one thing, as many as
  // entries it went back through, not everything over the entry it lands on.
  const short = async (script = SCRIPT) => {
    const first = await backPage();
    await press(first, first.el("scroll-area"), "3", { ctrlKey: true });
    await first.settle();
    await openerOf(first.el("side-cards").children.find((li) => li.hasAttribute("data-card"))).click();
    await first.settle();
    await first.setVisibility("hidden");
    const data = threadData();
    const page = newPage(first.storage, script, (p) => {
      p.enableHistory({ state: earlier(1, "thread"), below: [earlier(0, "top")] });
      p.setUserActivation({ hasBeenActive: false });
      p.threadingSupported = true;
      p.threads = data.threads;
      p.messages = data.messages;
    });
    await page.settle();
    await page.settle();
    const reopened = { facts: backFacts(page), pushes: page.history.pushes, thread: !page.el("thread-heading").hidden };
    await page.browserBack();
    return { reopened, after: { layout: layoutOf(page), thread: !page.el("thread-heading").hidden, left: page.left } };
  };
  const shortOne = await short();
  assert.equal(shortOne.reopened.thread, true, "fixture: the reload did not reopen the thread");
  assert.deepStrictEqual(shortOne.reopened.facts, { depth: 1, tag: "list" }, "the reopened list does not stand on the earlier load's entry");
  assert.equal(shortOne.reopened.pushes, 0, "an entry was pushed before the reader touched the page");
  assert.equal(shortOne.after.left, false);
  assert.equal(shortOne.after.thread, false, "the Back did not close the thread");
  assert.equal(shortOne.after.layout, "list", "a Back over an entry still waiting closed more than one thing");

  // AN ENTRY NOBODY CAN ACCOUNT FOR under the page's base — someone else's, holding no state — reached by
  // a Back while the thread list and a thread from it still wait for their entries: one thing closes.
  const foreign = async (script = SCRIPT) => {
    const activation = { hasBeenActive: false };
    const page = await backPage({ script, history: { below: [null] }, arrange: (p) => p.setUserActivation(activation) });
    await press(page, page.el("scroll-area"), "3", { ctrlKey: true });
    await page.settle();
    await openerOf(page.el("side-cards").children.find((li) => li.hasAttribute("data-card"))).click();
    await page.settle();
    const waiting = { facts: backFacts(page), thread: !page.el("thread-heading").hidden };
    await page.browserBack();
    return { page, waiting, after: { layout: layoutOf(page), thread: !page.el("thread-heading").hidden, left: page.left,
      facts: page.left ? null : backFacts(page) } };
  };
  const someone = await foreign();
  assert.equal(someone.waiting.thread, true, "fixture: the card did not open its thread");
  assert.deepStrictEqual(someone.waiting.facts, { depth: 0, tag: "top" }, "fixture: an entry was pushed before a touch");
  assert.equal(someone.after.left, false);
  assert.equal(someone.after.thread, false, "a Back onto someone else's entry did not close the thread");
  assert.equal(someone.after.layout, "list", "a Back onto someone else's entry closed more than the thread");
  assert.deepStrictEqual(someone.after.facts, { depth: 1, tag: "list" }, "someone else's entry was not made this load's, standing for the list");

  // NEGATIVE CONTROLS. The page as this change first made it — the entry reloaded on its base at depth
  // nought, and a Back onto an entry it did not make closing everything — closes the thread and the
  // list in the one press before any touch; with only the base at depth nought, in an installed app the
  // Backs after the last thing closes are spent on the earlier load's entries.
  const firstMade = brokenScript("const depth = earlier ? Math.min(earlier.depth, index === null ? BACK_DEPTH_MAX : index, BACK_DEPTH_MAX) : 0;",
    "const depth = 0;").replace("    target = Math.max(0, wasOpen - 1);\n", "    target = 0;\n");
  assert.ok(!firstMade.includes("    target = Math.max(0, wasOpen - 1);\n"), "the negative control's second patch found nothing");
  const greedy = await reloadWalk({ script: firstMade });
  assert.equal(greedy.gestures[0].thread, false);
  assert.equal(greedy.gestures[0].layout, null, "the control that closes everything on an earlier load's entry kept the list");
  assert.equal(greedy.gestures[0].picker, "flat");
  const stale = await reloadWalk({ leading: false, activation: null, script: brokenScript(
    "const depth = earlier ? Math.min(earlier.depth, index === null ? BACK_DEPTH_MAX : index, BACK_DEPTH_MAX) : 0;", "const depth = 0;") });
  assert.equal(stale.gestures[1].layout, null, "fixture: the control's second Back did not close the list");
  assert.ok(stale.gestures[1].index > 0, "the control that reloads at depth nought left no entry under its base");
  assert.ok(stale.gestures.length > 3, "the control's presses after the last thing closed were not spent on stale entries");
  // ...and a page that closes down to the depth of the entry landed on, however many things wait over it,
  // closes the list with the thread.
  const deep = await short(brokenScript("closeTo = Math.max(target, wasOpen - Math.max(0, from - target));", "closeTo = target;"));
  assert.equal(deep.after.layout, null, "the control that closes down to the entry's depth kept the list");
  const unadopted = await jumpedFrom(brokenScript("return earlier && earlier.depth < backAdopted ? earlier : null;", "return null;"));
  assert.equal(unadopted.layout, "list", "the control that takes no earlier load's entry for its depth closed both on a jump");
  const everything = await foreign(brokenScript("    target = Math.max(0, wasOpen - 1);\n", "    target = 0;\n"));
  assert.equal(everything.after.layout, null, "the control that closes everything on someone else's entry kept the list");
});

/** The view keys pressed in turn from the list, with how many entries each pushed and what it asked to go back through. */
async function viewKeyWalk(script = SCRIPT) {
  const page = await backPage({ script });
  page.wideDesk = true;
  const seen = [];
  for (const key of ["1", "2", "4", "1", "2", "4", "3", "1"]) {
    const pushes = page.history.pushes;
    await press(page, focused(page) || page.el("scroll-area"), key, { ctrlKey: true });
    await page.settle();
    seen.push({ key, pushed: page.history.pushes - pushes, asked: [...page.history.asked], layout: layoutOf(page),
      picker: page.el("thread-select").value, facts: backFacts(page) });
    await page.traverse();
  }
  return { page, seen };
}

test("`#304 back-gesture-in-app`: CTRL+1 TO CTRL+4 between Main, All and two columns push nothing; the thread list is one entry, and the key that leaves it goes back through it", async () => {
  const { page, seen } = await viewKeyWalk();
  for (const step of seen.slice(0, 6)) {
    assert.equal(step.pushed, 0, `Ctrl+${step.key} pushed an entry: ${JSON.stringify(step)}`);
    assert.deepStrictEqual(step.asked, [], `Ctrl+${step.key} went back through an entry`);
    assert.deepStrictEqual(step.facts, { depth: 0, tag: "top" }, `Ctrl+${step.key} left the page off its base`);
  }
  assert.deepStrictEqual(seen.slice(0, 6).map((step) => step.picker), ["main", "flat", "columns", "main", "flat", "columns"],
    "fixture: the view keys did not choose their views");
  const [list, out] = seen.slice(6);
  assert.equal(list.layout, "list");
  assert.equal(list.pushed, 1, "Ctrl+3 did not push the thread list's entry");
  assert.deepStrictEqual(list.facts, { depth: 1, tag: "list" });
  assert.equal(out.picker, "main");
  assert.equal(out.pushed, 0);
  assert.deepStrictEqual(out.asked, [-1], "Ctrl+1 on the thread list did not go back through its entry");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
  await page.browserBack();
  assert.equal(page.left, true, "a gesture after the view keys did not leave the app");

  // From a thread, Ctrl+3 is the list over it, where Esc on the list goes back to: one entry more, and
  // the gesture there is the thread again, then the channel.
  const thread = await backPage();
  await openFirstThread(thread);
  await press(thread, thread.el("scroll-area"), "3", { ctrlKey: true });
  await thread.settle();
  assert.deepStrictEqual(backFacts(thread), { depth: 2, tag: "list" }, "the list chosen over a thread is not an entry above it");
  await thread.browserBack();
  assert.equal(layoutOf(thread), null);
  assert.equal(thread.el("thread-select").value, `thread:${thread.threads[0].id}`, "the gesture on the list did not go back to the thread");
  assert.deepStrictEqual(backFacts(thread), { depth: 1, tag: "thread" });
  await thread.browserBack();
  assert.equal(thread.el("thread-select").value, "flat");
  assert.equal(thread.left, false);

  // NEGATIVE CONTROL: a page that takes every view but Main for a thread pushes for All.
  const pushy = await viewKeyWalk(brokenScript('} else if (selection().view === "thread") {', '} else if (selection().view !== "main") {'));
  assert.ok(pushy.seen.slice(0, 6).some((step) => step.pushed > 0), "the control that takes All for a screen pushed nothing");
});

test("`#304 back-gesture-in-app`: Settings, Help over it, and the reply screen are entries; the gesture closes each as its Back does, the draft kept", async () => {
  const page = await backPage();
  await page.el("open-settings").click();
  await page.settle();
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "settings" });
  await page.el("help-link-microphone").click();
  await page.settle();
  assert.equal(page.screen(), "help");
  assert.deepStrictEqual(backFacts(page), { depth: 2, tag: "help" });
  await page.browserBack();
  assert.equal(page.screen(), "settings", "the gesture in Help did not go back to Settings");
  await page.browserBack();
  assert.equal(page.screen(), "main", "the gesture in Settings did not go back to the channel");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
  // "?" from a thread: Help over the thread, and the gesture there goes back to the thread, then out of it.
  await openFirstThread(page);
  await press(page, page.el("scroll-area"), "?");
  await page.settle();
  assert.equal(page.screen(), "help");
  assert.deepStrictEqual(backFacts(page), { depth: 2, tag: "help" });
  await page.browserBack();
  assert.equal(page.screen(), "main");
  assert.equal(page.el("thread-heading").hidden, false, "the gesture in Help opened over a thread closed the thread too");
  await page.browserBack();
  assert.equal(page.el("thread-heading").hidden, true);
  // The reply screen: the gesture puts it down as Cancel does, and the words are kept as a draft.
  const answered = page.el("discord-log").children[0];
  await replyButton(answered).click();
  await page.settle();
  assert.ok(replyInStream(page));
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "reply" });
  await page.el("reply-text").setValue("half a thought");
  await page.browserBack();
  assert.equal(page.screen(), "main", "the gesture did not leave the reply screen");
  assert.equal(page.left, false);
  await replyButton(page.el("discord-log").children[0]).click();
  assert.equal(page.el("reply-text").value, "half a thought", "the gesture on the reply screen lost the draft");
});

test("`#304 back-gesture-in-app`: in two columns the right column's thread is an entry; the gesture is its X, and the X goes back through it", async () => {
  const page = await backPage();
  page.wideDesk = true;
  await press(page, page.el("scroll-area"), "4", { ctrlKey: true });
  await page.settle();
  assert.equal(layoutOf(page), "two");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" }, "two columns over the cards pushed an entry");
  const card = page.el("side-cards").children.find((li) => li.hasAttribute("data-card"));
  await openerOf(card).click();
  await page.settle();
  assert.ok(!cardsUp(page), "fixture: the card did not open its thread on the right");
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "side" });
  await page.browserBack();
  assert.ok(cardsUp(page), "the gesture did not take the right column back to its cards");
  assert.equal(layoutOf(page), "two", "the gesture left two columns");
  assert.equal(page.left, false);
  await openerOf(page.el("side-cards").children.find((li) => li.hasAttribute("data-card"))).click();
  await page.settle();
  await page.el("side-close").click();
  await page.settle();
  assert.ok(cardsUp(page));
  assert.deepStrictEqual(page.history.asked, [-1], "the X did not go back through the thread's entry");
});

/** A row's ⋯ menu opened, on `script`, with close watchers where `closeWatcher` says. */
async function menuPage(closeWatcher, script = SCRIPT) {
  const page = await backPage({ script, closeWatcher });
  const row = page.el("discord-log").children[0];
  await rowMoreButton(row).click();
  await page.settle();
  return { page, row };
}

/**
 * The ⋯ menu of a row opened over something a Back closes, in a window that has close watchers, then a
 * plain Back — a desk's toolbar Back, a mouse's back button, Alt+Left, which no close watcher hears —
 * and another: one column with a thread open, or (`two`) two columns with a thread on the right and the
 * menu on a row of the left column's. What each step came to, on `script`.
 */
async function menuOverLevel(two, script = SCRIPT) {
  const page = await backPage({ script, closeWatcher: true });
  const note = () => ({ facts: page.left ? null : backFacts(page), thread: !page.el("thread-heading").hidden,
    cards: cardsUp(page), side: sideIds(page), left: page.left, watching: page.watching().length });
  if (two) {
    await twoColumns(page);
    await threadBadge(rowWithId(page, "201")).click();
    await page.settle();
  } else {
    await openFirstThread(page);
  }
  const under = note();
  const row = page.el("discord-log").children[0];
  await rowMoreButton(row).click();
  await page.settle();
  const opened = { ...note(), menu: !rowMoreMenu(row).hidden };
  await page.browserBack();
  const first = { ...note(), menu: !rowMoreMenu(row).hidden };
  await page.browserBack();
  const second = { ...note(), menu: !rowMoreMenu(row).hidden };
  return { page, row, under, opened, first, second };
}

test("`#304 back-gesture-in-app`: A MENU IS AN ENTRY even where the browser has close watchers, so a plain Back closes the menu alone — never the thread under it, in one column or on the right of two — and the next Back that thread", async () => {
  // One column: the ⋯ menu over a thread.
  const one = await menuOverLevel(false);
  assert.deepStrictEqual(one.under.facts, { depth: 1, tag: "thread" });
  assert.equal(one.opened.menu, true, "fixture: the ⋯ did not open its menu");
  assert.deepStrictEqual(one.opened.facts, { depth: 2, tag: "menu:row" }, "the ⋯ menu over a thread is not an entry over the thread's");
  assert.equal(one.opened.watching, 0, "the page made a close watcher, which a desk's Back passes by");
  assert.equal(one.first.menu, false, "a plain Back did not close the ⋯ menu");
  assert.equal(one.first.thread, true, "a plain Back with the ⋯ menu open closed the thread under it");
  assert.deepStrictEqual(one.first.facts, { depth: 1, tag: "thread" });
  assert.equal(one.second.thread, false, "the next Back did not close the thread");
  assert.equal(one.second.left, false);
  // The same thread again: no menu is still open on it.
  await openFirstThread(one.page);
  assert.ok(one.page.el("discord-log").children.every((li) => !rowMoreMenu(li) || rowMoreMenu(li).hidden),
    "a menu is still open in the thread opened again");
  assert.deepStrictEqual(backFacts(one.page), { depth: 1, tag: "thread" });

  // Two columns: the ⋯ menu on a row of the left column's, a thread on the right.
  const two = await menuOverLevel(true);
  assert.deepStrictEqual(two.under.facts, { depth: 1, tag: "side" });
  assert.equal(two.under.cards, false, "fixture: the root's tag did not open its thread on the right");
  assert.deepStrictEqual(two.opened.facts, { depth: 2, tag: "menu:row" });
  assert.equal(two.first.menu, false, "a plain Back did not close the ⋯ menu in two columns");
  assert.equal(two.first.cards, false, "a plain Back with the ⋯ menu open closed the right column's thread");
  assert.deepStrictEqual(two.first.side, two.under.side, "the right column's thread changed under the menu's Back");
  assert.deepStrictEqual(two.first.facts, { depth: 1, tag: "side" });
  assert.equal(two.second.cards, true, "the next Back did not take the right column back to its cards");
  assert.equal(two.second.left, false);

  // Esc and a tap outside close the menu at once, and go back through its entry.
  for (const [what, close] of [
    ["Esc", (p, row) => press(p, rowMoreButton(row), "Escape")],
    ["a tap outside", (p) => p.documentEvent("pointerdown", { target: p.el("scroll-area"), pointerType: "touch" })],
  ]) {
    const page = await backPage({ closeWatcher: true });
    await openFirstThread(page);
    const row = page.el("discord-log").children[0];
    await rowMoreButton(row).click();
    await page.settle();
    assert.deepStrictEqual(backFacts(page), { depth: 2, tag: "menu:row" });
    await close(page, row);
    await page.settle();
    assert.equal(rowMoreMenu(row).hidden, true, `${what} did not close the menu`);
    assert.deepStrictEqual(page.history.asked, [-1], `${what} did not go back through the menu's entry`);
    await page.traverse();
    assert.equal(page.el("thread-heading").hidden, false, `${what}: the page's own going-back closed the thread`);
    assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "thread" });
  }

  // NEGATIVE CONTROL: the page as this change first made it, where a browser with close watchers gives
  // a menu no entry: a plain Back closes the thread under the menu, and the menu stays open.
  const unlisted = await menuOverLevel(false, brokenScript("  levels.push(...openMenus());",
    "  if (typeof window.CloseWatcher !== \"function\") levels.push(...openMenus());"));
  assert.deepStrictEqual(unlisted.opened.facts, { depth: 1, tag: "thread" }, "the control gave the menu an entry");
  assert.equal(unlisted.first.thread, false, "the control's Back did not close the thread under the menu");
  assert.equal(unlisted.first.menu, true, "the control's Back closed the menu");
});

/**
 * The reading-pace popover opened by its button, with close watchers or without, on `script`; then the
 * gesture, then the popover opened and closed by its button, and its going-back landed.
 */
async function paceWalk(closeWatcher, script = SCRIPT) {
  const page = await backPage({ script, closeWatcher });
  await page.el("read-speed").click();
  await page.settle();
  const opened = { open: !page.el("speed-popover").hidden, facts: backFacts(page), watching: page.watching().length };
  const how = await page.gestureBack();
  const gesture = { how, open: !page.el("speed-popover").hidden, facts: page.left ? null : backFacts(page),
    left: page.left, focus: focusName(page) };
  if (page.left) return { page, opened, gesture };
  await page.el("read-speed").click();
  await page.settle();
  await press(page, page.el("read-speed"), "Escape");
  await page.settle();
  const escaped = { open: !page.el("speed-popover").hidden, asked: [...page.history.asked] };
  await page.traverse();
  return { page, opened, gesture, escaped };
}

test("`#304 back-gesture-in-app`: THE READING-PACE POPOVER is an entry, with close watchers or without: the gesture closes it, the focus on its button, and Esc closing it goes back through its entry", async () => {
  for (const closeWatcher of [false, true]) {
    const how = closeWatcher ? "with close watchers" : "without close watchers";
    const { page, opened, gesture, escaped } = await paceWalk(closeWatcher);
    assert.equal(opened.open, true, `${how}: fixture: the pace button did not open its popover`);
    assert.deepStrictEqual(opened.facts, { depth: 1, tag: "menu:pace" }, `${how}: the open popover is not an entry`);
    assert.equal(opened.watching, 0, `${how}: the popover has a close watcher, which a desk's Back passes by`);
    assert.equal(gesture.how, "history");
    assert.equal(gesture.left, false, `${how}: the gesture with the popover open left the app`);
    assert.equal(gesture.open, false, `${how}: the gesture did not close the popover`);
    assert.equal(gesture.focus, "#read-speed", `${how}: the gesture did not give the focus to the pace button`);
    assert.deepStrictEqual(gesture.facts, { depth: 0, tag: "top" });
    assert.equal(escaped.open, false, `${how}: Esc did not close the popover`);
    assert.deepStrictEqual(escaped.asked, [-1], `${how}: Esc did not go back through the popover's entry`);
    assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
    await page.browserBack();
    assert.equal(page.left, true, `${how}: after the popover, the gesture at the top did not leave the app`);
  }
  // NEGATIVE CONTROLS: a pace button that never tells the page it opened the popover gives it no entry,
  // and the gesture leaves the app with it open; a popover put down without telling the page leaves its
  // entry under the next gesture.
  for (const closeWatcher of [false, true]) {
    const silent = await paceWalk(closeWatcher, brokenScript(
      "  // `#304 back-gesture-in-app`. An open popover is a level, with an entry of its own.\n  scheduleBackSync();\n", ""));
    assert.equal(silent.gesture.left, true, "the control whose pace button pushes nothing did not leave the app");
    assert.equal(silent.gesture.open, true);
  }
  const quiet = await paceWalk(false, brokenScript('  el("read-speed").setAttribute("aria-expanded", "false");\n  scheduleBackSync();\n',
    '  el("read-speed").setAttribute("aria-expanded", "false");\n'));
  assert.deepStrictEqual(quiet.escaped.asked, [], "the control that closes the popover silently went back through its entry");
});

/** Two columns on a page keeping a history, the first thread on the right, and a reply opened at the right column's foot. */
async function dockedReplyWalk(script = SCRIPT) {
  const page = newPage(new Map(), script, (p) => p.enableHistory());
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  forwardTimeline(page);
  await signIn(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  await page.settle();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  const side = { facts: backFacts(page), ids: sideIds(page) };
  await replyButton(sideRow(page, "202")).click();
  await page.settle();
  const docked = { facts: backFacts(page), at: replyBoxAt(page) };
  await page.el("reply-text").setValue("draft words");
  await page.browserBack();
  const gesture = { facts: page.left ? null : backFacts(page), at: replyBoxAt(page), ids: sideIds(page), cards: cardsUp(page),
    left: page.left, screen: page.screen() };
  if (page.left || gesture.cards) return { page, side, docked, gesture };
  await replyButton(sideRow(page, "202")).click();
  await page.settle();
  const again = { facts: backFacts(page), at: replyBoxAt(page), text: page.el("reply-text").value };
  await page.el("reply-cancel").click();
  await page.settle();
  const cancelled = { at: replyBoxAt(page), asked: [...page.history.asked] };
  await page.traverse();
  const landed = { facts: backFacts(page), ids: sideIds(page), cards: cardsUp(page) };
  return { page, side, docked, gesture, again, cancelled, landed };
}

test("`#304 back-gesture-in-app`: A REPLY OPEN AT THE FOOT OF THE RIGHT COLUMN is an entry over its thread's: the gesture puts it down with the draft kept and the thread still on the right, and Cancel goes back through its entry", async () => {
  const { page, side, docked, gesture, again, cancelled, landed } = await dockedReplyWalk();
  assert.deepStrictEqual(side.facts, { depth: 1, tag: "side" });
  assert.deepStrictEqual(side.ids, ["201", "202"], "fixture: the first thread is not on the right");
  assert.equal(docked.at, "right", "fixture: Reply in the right column did not open its box at that column's foot");
  assert.deepStrictEqual(docked.facts, { depth: 2, tag: "reply" }, "the reply open in a column is not an entry over its thread's");
  assert.equal(gesture.left, false, "the gesture with a reply open in a column left the app");
  assert.notEqual(gesture.at, "right", "the gesture did not put the reply down");
  assert.equal(gesture.screen, "main");
  assert.equal(gesture.cards, false, "the gesture that put the reply down closed the right column's thread too");
  assert.deepStrictEqual(gesture.ids, ["201", "202"]);
  assert.deepStrictEqual(gesture.facts, { depth: 1, tag: "side" });
  assert.equal(again.text, "draft words", "the gesture lost the reply's words");
  assert.deepStrictEqual(again.facts, { depth: 2, tag: "reply" });
  assert.notEqual(cancelled.at, "right", "Cancel did not put the reply down");
  assert.deepStrictEqual(cancelled.asked, [-1], "Cancel did not go back through the reply's entry");
  assert.deepStrictEqual(landed.facts, { depth: 1, tag: "side" });
  assert.equal(landed.cards, false, "the page's own going-back after Cancel closed the right column's thread");
  assert.deepStrictEqual(landed.ids, ["201", "202"]);
  await page.browserBack();
  assert.equal(cardsUp(page), true, "the gesture after the reply did not take the right column back to its cards");
  assert.equal(page.left, false);

  // NEGATIVE CONTROLS. A page that takes no reply open in a column for a level closes the thread under
  // it with the one gesture; one never told a reply opened gives it no entry; one never told it was put
  // down leaves its entry for the next gesture to spend.
  const unlevelled = await dockedReplyWalk(brokenScript('  if (replyDocked) levels.push("reply");\n', "\n"));
  assert.deepStrictEqual(unlevelled.docked.facts, { depth: 1, tag: "side" }, "the control with no reply level pushed an entry for it");
  assert.equal(unlevelled.gesture.cards, true, "the control with no reply level kept the right column's thread on the gesture");
  const untold = await dockedReplyWalk(brokenScript(
    "  // `#304 back-gesture-in-app`. A reply open in the stream is a level of its own.\n  scheduleBackSync();\n", ""));
  assert.deepStrictEqual(untold.docked.facts, { depth: 1, tag: "side" }, "the control never told a reply opened pushed an entry for it");
  const undocked = await dockedReplyWalk(brokenScript("  renderReplyingMarks();\n  markCardBesideReply();\n  scheduleBackSync();\n}",
    "  renderReplyingMarks();\n  markCardBesideReply();\n}"));
  assert.deepStrictEqual(undocked.cancelled.asked, [], "the control never told the reply was put down went back through its entry");
});

test("`#304 back-gesture-in-app`: without close watchers a menu is an entry: the gesture closes it, and a tap outside goes back through it", async () => {
  const { page, row } = await menuPage(false);
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "menu:row" });
  await page.browserBack();
  assert.equal(rowMoreMenu(row).hidden, true, "the gesture did not close the menu");
  assert.equal(page.left, false);
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" });
  await rowMoreButton(row).click();
  await page.settle();
  await page.documentEvent("pointerdown", { target: page.el("scroll-area"), pointerType: "touch" });
  await page.settle();
  assert.equal(rowMoreMenu(row).hidden, true);
  assert.deepStrictEqual(page.history.asked, [-1], "a tap outside the menu did not go back through its entry");
  // The pace popover and the prompts tray are menus too, in the order Esc closes them.
  await page.traverse();
  await page.el("prompts-open").click();
  await page.settle();
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "menu:prompts" });
  await page.browserBack();
  assert.equal(page.el("prompts-tray").hidden, true, "the gesture did not close the prompts tray");
});

test("`#304 back-gesture-in-app`: an entry wanted before the reader has touched the page waits for their first touch, as Chromium skips one pushed earlier", async () => {
  const activation = { hasBeenActive: false };
  const page = await backPage({ arrange: (p) => p.setUserActivation(activation) });
  await openFirstThread(page);
  assert.equal(page.el("thread-heading").hidden, false);
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" }, "an entry was pushed before the reader touched the page");
  activation.hasBeenActive = true;
  await page.documentEvent("pointerup", { target: page.el("scroll-area"), pointerType: "touch" });
  await page.settle();
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "thread" }, "the reader's first touch did not push the waiting entry");
  await page.browserBack();
  assert.equal(page.el("thread-heading").hidden, true);
});

test("`#304 back-gesture-in-app`: a Back that cannot close what is open never traps the reader: no entry is pushed for it again, and the next gesture leaves", async () => {
  // A page whose thread the gesture cannot close, made so on purpose: the safety net is what is tested.
  const page = await backPage({ script: brokenScript('else if (tag === "thread") leaveThread();', "") });
  await openFirstThread(page);
  assert.deepStrictEqual(backFacts(page), { depth: 1, tag: "thread" });
  await page.browserBack();
  assert.equal(page.el("thread-heading").hidden, false, "fixture: the broken page closed the thread");
  assert.deepStrictEqual(backFacts(page), { depth: 0, tag: "top" }, "an entry was pushed again for what could not close");
  await page.browserBack();
  assert.equal(page.left, true, "the reader was trapped in a thread the gesture cannot close");
});

/** What a channel picker holds: each service header with its channels' ids, or bare ids. */
function pickerShape(page, id = "discord-channel") {
  return page.el(id).children.map((child) => String(child.tagName).toLowerCase() === "optgroup"
    ? { group: child.getAttribute("label"), channels: child.children.map((option) => String(option.value)) }
    : String(child.value));
}

test("`#355 provider-grouped-channels`: with two services, the bar's picker groups its channels under a header per service, in the server's order, and a cold start draws them before client-config answers", async () => {
  const page = newPage();
  twoServices(page);
  await signIn(page);
  const grouped = [
    { group: "Slack", channels: [TEAM_A.id, TEAM_B.id] },
    { group: "Google Chat", channels: [SPACE_A.id, SPACE_B.id] },
  ];
  assert.deepStrictEqual(pickerShape(page), grouped, "the bar's picker was not grouped by service in configuration order");
  assert.equal(String(page.el("discord-channel").value), TEAM_A.id,
    "with nothing stored, the picker opened on the server's first channel rather than the first one shown");
  // The closed picker shows a channel's name alone; the header is the open list's.
  const option = page.el("discord-channel").children[1].children[0];
  assert.equal(option.textContent, "space a");
  // Names still come from the list, now that the picker holds groups. An id no longer listed (an
  // outbox entry may outlive its channel) is named by its id, as always.
  const channelLabel = page.pure("channelLabel");
  assert.equal(channelLabel(SPACE_B.id), "space b", "a channel inside a group lost its name");
  assert.equal(channelLabel("1110000000000000999"), "1110000000000000999");

  page.el("discord-channel").value = SPACE_B.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  // A RELOAD over the same storage: the saved shell draws the groups before client-config answers.
  const reloaded = newPage(page.storage, SCRIPT, (next) => twoServices(next));
  assert.deepStrictEqual(pickerShape(reloaded), grouped, "a cold start drew the picker flat");
  assert.equal(String(reloaded.el("discord-channel").value), SPACE_B.id, "the stored channel was not restored inside its group");
  await reloaded.settle();
  assert.deepStrictEqual(pickerShape(reloaded), grouped);
  assert.equal(String(reloaded.el("discord-channel").value), SPACE_B.id);
});

test("`#355 provider-grouped-channels`: no headers while only one service has channels", async () => {
  for (const [label, arrange] of [
    ["one service", (page) => {
      page.channels = [{ ...TEAM_A }, { ...TEAM_B }];
      page.providers = [serviceOf("slack", "Slack")];
    }],
    ["two services, channels in one", (page) => {
      page.channels = [{ ...TEAM_A }, { ...TEAM_B }];
      page.providers = [serviceOf("slack", "Slack"), serviceOf("gchat", "Google Chat")];
    }],
    ["a server that describes no services", (page) => {
      page.channels = [{ ...TEAM_A }, { ...SPACE_A }];
    }],
  ]) {
    const page = newPage();
    arrange(page);
    await signIn(page);
    assert.deepStrictEqual(pickerShape(page), page.channels.map((channel) => channel.id), `${label}: headers drawn`);
    assert.equal(String(page.el("discord-channel").value), page.channels[0].id, label);
  }
});

test("`#355 provider-grouped-channels`: services sharing a name are told apart by key, and channels of no described service come last, not dropped", async () => {
  const page = newPage();
  const home = { id: "C0500HOMEA", label: "home team", writable: true, alias: null, added: false, provider: "home" };
  const stray = { id: "1110000000000000403", label: "old bridge room", writable: false, alias: null, added: true, provider: "gone" };
  page.channels = [{ ...stray }, { ...home }, { ...TEAM_A }];
  page.providers = [serviceOf("slack", "Slack"), serviceOf("home", "Slack")];
  await signIn(page);
  assert.deepStrictEqual(pickerShape(page), [
    { group: "Slack (slack)", channels: [TEAM_A.id] },
    { group: "Slack (home)", channels: [home.id] },
    { group: "Not connected", channels: [stray.id] },
  ]);
});

test("`#355 provider-grouped-channels`: Alt+↓ and Alt+↑ step through the channels in the order the grouped picker shows, across groups", async () => {
  const page = newPage();
  twoServices(page);
  await signIn(page);
  await showDiscord(page, tallChannel(5));
  const picker = page.el("discord-channel");
  assert.equal(String(picker.value), TEAM_A.id);
  const step = async (key) => {
    await press(page, focused(page) || page.el("scroll-area"), key, { altKey: true });
    await page.settle();
    return String(picker.value);
  };
  assert.equal(await step("ArrowDown"), TEAM_B.id);
  // The server lists Slack's last channel last; the picker shows Google Chat's after it.
  assert.equal(await step("ArrowDown"), SPACE_A.id, "Alt+↓ stopped at the last channel of the first group, in the server's order");
  assert.equal(await step("ArrowDown"), SPACE_B.id);
  assert.equal(await step("ArrowDown"), SPACE_B.id, "Alt+↓ at the last channel shown did not stop");
  assert.equal(await step("ArrowUp"), SPACE_A.id);
  assert.equal(await step("ArrowUp"), TEAM_B.id, "Alt+↑ did not cross back into the first group");
});

test("`#355 provider-grouped-channels`: with one service, Settings has no Service picker and asks the server what it always asked", async () => {
  const page = browsingPage();
  page.chatProviderName = "Slack";
  await signIn(page);
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("settings-service-row").hidden, true, "a Service picker for one service");
  assert.equal(page.el("chat-provider-line").hidden, false);
  assert.equal(page.el("chat-provider-settings").textContent, "Slack");
  await page.el("open-browse-channels").click();
  await page.settle();
  assert.equal(page.directoryCalls[0].searchParams.get("provider"), null, "a one-service browse named a service");
  await page.el("open-add-channel").click();
  page.el("new-channel-id").value = "https://chat.example/room/7";
  page.el("new-channel-label").value = "seventh";
  await page.el("add-channel").click();
  await page.settle();
  assert.deepStrictEqual(page.addChannelCalls.at(-1), { source: "https://chat.example/room/7", label: "seventh" });
});

test("`#355 provider-grouped-channels`: with two services, Settings chooses the service first, opens on the one being read, and lists only its channels", async () => {
  const page = await servicesPage();
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("settings-service-row").hidden, false);
  assert.equal(page.el("chat-provider-line").hidden, true, "the one-service line stayed beside the Service picker");
  assert.deepStrictEqual(page.el("settings-service").children.map((option) => [String(option.value), option.textContent]),
    [["slack", "Slack"], ["gchat", "Google Chat"]], "the services are not in configuration order");
  // The bar opened on Slack's first channel, so Settings opens on Slack.
  assert.equal(page.el("settings-service").value, "slack");
  assert.ok(!settingsOptions(page).includes(SPACE_A.id) && settingsOptions(page).includes(TEAM_A.id)
    && settingsOptions(page).includes(TEAM_B.id), `Settings listed ${settingsOptions(page)}`);
  assert.match(page.el("service-facts").textContent, /^Slack: New messages are checked for every 15 seconds\. Threads are shown\. Add a channel by pasting its link, or browse its channels\. Unread can be cleared in Slack itself\.$/);
  await chooseService(page, "gchat");
  assert.deepStrictEqual(settingsOptions(page).filter((id) => id !== "*"), [SPACE_A.id, SPACE_B.id]);
  assert.equal(String(page.el("settings-channel").value), SPACE_A.id);
  assert.match(page.el("service-facts").textContent, /^Google Chat: New messages arrive as they are sent\. No threads\. Add a channel by its ID\. Unread cannot be cleared in Google Chat from here\.$/);
  assert.equal(page.el("open-browse-channels").hidden, true, "Browse offered for a service that cannot list channels");
  assert.equal(page.el("new-channel-writable-row").hidden, false, "a direct service hid Let this app post here");
  assert.equal(page.el("new-channel-id").placeholder, "the channel's ID");

  // Help and back is the same visit: the service chosen stands.
  await page.el("help-link-channel-alias").click();
  await page.settle();
  await page.el("close-help").click();
  await page.settle();
  assert.equal(page.screen(), "settings");
  assert.equal(page.el("settings-service").value, "gchat", "the way back from Help undid the reader's choice");
  // Leaving and opening Settings again goes back to the service of the channel being read.
  await page.el("close-settings").click();
  await page.settle();
  page.el("discord-channel").value = SPACE_B.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("settings-service").value, "gchat");
  await chooseService(page, "slack");
  await page.el("close-settings").click();
  await page.settle();
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("settings-service").value, "gchat", "Settings did not open on the service of the channel being read");
  assert.equal(String(page.el("settings-channel").value), SPACE_B.id, "Settings did not open on the channel being read");
});

test("`#355 provider-grouped-channels`: Add and Browse act on the service Settings shows, and the answer selects the new channel and its service", async () => {
  const page = await servicesPage((page) => {
    page.directory = [{ source: "slack:C0400CCCCC", name: "#design", channel_id: null }];
  });
  await page.el("open-settings").click();
  await page.settle();
  // Slack, which registers: a link, with its service.
  await page.el("open-add-channel").click();
  page.el("new-channel-id").value = "https://acme.slack.com/archives/C0400DDDDD";
  page.el("new-channel-label").value = "added";
  await page.el("add-channel").click();
  await page.settle();
  assert.deepStrictEqual(page.addChannelCalls.at(-1),
    { source: "https://acme.slack.com/archives/C0400DDDDD", label: "added", provider: "slack" });
  assert.equal(page.el("settings-service").value, "slack");
  assert.equal(String(page.el("settings-channel").value), page.registeredChannelId);
  // Browse asks for Slack's channels only, and its Add names Slack too.
  await page.el("open-browse-channels").click();
  await page.settle();
  assert.equal(page.directoryCalls.at(-1).searchParams.get("provider"), "slack");
  assert.equal(page.directoryCalls.at(-1).searchParams.get("limit"), "25");
  assert.deepStrictEqual(directoryRows(page), ["#design | Add"]);
  // Another service empties the list and closes the panel: they were Slack's.
  await chooseService(page, "gchat");
  assert.equal(page.el("browse-channel-fields").hidden, true);
  assert.equal(page.el("channel-directory-list").children.length, 0, "Slack's channels stayed listed under Google Chat");
  // Google Chat adds by id: an id, a writable choice and its service; the answer selects it.
  await page.el("open-add-channel").click();
  page.el("new-channel-id").value = "1110000000000000499";
  page.el("new-channel-label").value = "by id";
  page.el("new-channel-writable").checked = true;
  page.channelRegistrationSupported = false;
  await page.el("add-channel").click();
  await page.settle();
  assert.deepStrictEqual(page.addChannelCalls.at(-1),
    { id: "1110000000000000499", label: "by id", writable: true, provider: "gchat" });
  assert.equal(page.el("settings-service").value, "gchat");
  assert.equal(String(page.el("settings-channel").value), "1110000000000000499");
});

test("`#355 provider-grouped-channels`: hidden channels are listed under their own service, and a service with no channels says how to add one", async () => {
  const hiddenTeam = { id: "C0400EEEEE", label: "old team", writable: false, provider: "slack" };
  const page = await servicesPage((page) => {
    page.channels = [{ ...TEAM_A }];
    page.hiddenChannels = [hiddenTeam];
  });
  await page.el("open-settings").click();
  await page.settle();
  const hiddenNames = () => page.el("hidden-channel-list").children.map((row) => row.children[0].textContent);
  assert.deepStrictEqual(hiddenNames(page), ["old team"]);
  assert.equal(page.el("hidden-channels").hidden, false);
  await chooseService(page, "gchat");
  assert.deepStrictEqual(hiddenNames(page), [], "Slack's hidden channel was offered under Google Chat");
  assert.equal(page.el("hidden-channels").hidden, true);
  assert.deepStrictEqual(settingsOptions(page), []);
  assert.equal(page.el("remove-channel").hidden, true, "Remove offered with no channel");
  assert.equal(page.el("channel-facts").textContent, "No Google Chat channels on your list yet. Add one.");
  await chooseService(page, "slack");
  // Slack's own channel, and its hidden one offered back; Show again puts it back under Slack.
  const show = page.el("hidden-channel-list").children[0].children[1];
  await show.click();
  await page.settle();
  assert.equal(page.el("settings-service").value, "slack");
  assert.equal(String(page.el("settings-channel").value), hiddenTeam.id);
});

test("`#355 provider-grouped-channels`: with two services, All channels for Clear unread is the chosen service's, and a Save never reaches another service's channels", async () => {
  const space3 = { id: "1110000000000000405", label: "space c", writable: true, alias: null, added: false, provider: "gchat" };
  const page = await servicesPage((page) => {
    page.channels = [{ ...SPACE_A }, { ...TEAM_A }, { ...SPACE_B }, { ...TEAM_B }, space3];
    page.providers[1] = { ...page.providers[1], upstream_clear_supported: true };
  });
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("settings-service").value, "slack");
  assert.deepStrictEqual(pickerChoices(page),
    [["*", "All Slack channels"], [TEAM_A.id, "team a"], [TEAM_B.id, "team b"]]);
  await settingsChannel(page, "*");
  assert.equal(page.el("channel-facts").textContent, "All 2 Slack channels on your list.");
  await page.settle();
  assert.deepStrictEqual([...new Set(page.upstreamClearCalls.filter((call) => call.startsWith("GET")))].sort(),
    [`GET ${TEAM_A.id}`, `GET ${TEAM_B.id}`], "All Slack channels read another service's setting");
  page.el("upstream-clear-mode").value = "on";
  await page.el("upstream-clear-mode").dispatch("change");
  await page.el("upstream-clear-save").click();
  await page.settle();
  assert.deepStrictEqual(page.upstreamClearCalls.filter((call) => call.startsWith("PUT")).map((call) => call.split(" ")[1]).sort(),
    [TEAM_A.id, TEAM_B.id], "a Save on All Slack channels reached another service's channels");
  // Google Chat's three, on the same terms.
  await chooseService(page, "gchat");
  assert.deepStrictEqual(pickerChoices(page).map(([value, words]) => value === "*" ? words : value),
    ["All Google Chat channels", SPACE_A.id, SPACE_B.id, space3.id]);
});

test("`#355 provider-grouped-channels`: All channels is not offered for a service whose channels cannot take Clear unread, though another service's can", async () => {
  const page = await servicesPage();
  await page.el("open-settings").click();
  await page.settle();
  await chooseService(page, "gchat");
  assert.ok(!pickerChoices(page).some(([value]) => value === "*"),
    "All channels offered for a service none of whose channels can clear unread");
  await chooseService(page, "slack");
  assert.equal(pickerChoices(page)[0][1], "All Slack channels");
});

test("`#355 provider-grouped-channels`: a channel of no service the server describes is reached in Settings under Not connected, where nothing can be added", async () => {
  const stray = { id: "1110000000000000406", label: "old bridge room", writable: false, alias: null, added: true, provider: null };
  const page = await servicesPage((page) => {
    page.channels = [{ ...TEAM_A }, { ...SPACE_A }, stray];
  });
  await page.el("open-settings").click();
  await page.settle();
  assert.deepStrictEqual(page.el("settings-service").children.map((option) => [String(option.value), option.textContent]),
    [["slack", "Slack"], ["gchat", "Google Chat"], ["", "Not connected"]]);
  await chooseService(page, "");
  assert.deepStrictEqual(settingsOptions(page), [stray.id], "the stray channel cannot be reached from Settings");
  assert.equal(page.el("open-add-channel").hidden, true, "Add offered for channels of no service");
  assert.equal(page.el("open-browse-channels").hidden, true);
  assert.match(page.el("service-facts").textContent, /no longer reads/);
  assert.match(page.el("channel-facts").textContent, /"old bridge room"/);
  await chooseService(page, "slack");
  assert.equal(page.el("open-add-channel").hidden, false);
  // Opened while the stray channel is read, Settings opens on it.
  await page.el("close-settings").click();
  await page.settle();
  page.el("discord-channel").value = stray.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("settings-service").value, "");
  assert.equal(String(page.el("settings-channel").value), stray.id);
});
