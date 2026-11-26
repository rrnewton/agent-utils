// `#2279 vibe-talk-post-facto-labels` (owner requests 380, 382): a label added to a message after
// the fact, from its ⋯ menu and the phone's long-press sheet. One label set: a menu-added `#perf`
// is the label a typed `#perf` makes; a plain name rides beside it. The fixture keeps the menu
// labels as the server does (tests/js/page/fixture.mjs, `labelRoute`), and the page's exact
// requests are asserted, not a stand-in call; the server route itself is proven through the real
// router in tests/message_labels.rs.

import assert from "node:assert/strict";

import { backlog, channelReads, channelRow, hold, message, newPage, reReadChannel, rowMoreButton, showDiscord, signIn, test, unfiltered } from "./fixture.mjs";

const labelItem = (li) => li.descendants().find((node) => node.className === "row-label-button");
const chipTexts = (li) => li.descendants().filter((node) => node.hasClass && node.hasClass("label-chip")).map((node) => node.textContent);
const pickerRows = (page) => page.el("label-picker-list").children;
const pickerRowNamed = (page, name) => pickerRows(page).find((li) => li.getAttribute("data-value") === name);

async function labelledPage({ threading = true, messages = null } = {}) {
  const page = newPage();
  page.threadingSupported = threading;
  page.serveLabelsRevision = true;
  await signIn(page);
  const held = messages || backlog(3);
  const rows = await showDiscord(page, held);
  return { page, rows, messages: held };
}

async function openPicker(page, row) {
  await rowMoreButton(row).click();
  await labelItem(row).click();
  await page.settle();
  await page.settle();
}

async function typeAndEnter(page, text) {
  await page.el("label-picker-input").setValue(text);
  await page.el("label-picker-input").dispatch("keydown", { key: "Enter", preventDefault: () => {} });
  await page.settle();
  await page.settle();
}

for (const threading of [true, false]) {
  test(`\`#2279\`: Add label… opens the picker, a new #name is added with the page's exact PUT, and the chip shows at once (threading ${threading})`, async () => {
    const { page, messages } = await labelledPage({ threading });
    const id = String(messages[1].id);
    await openPicker(page, channelRow(page, id));
    assert.equal(page.el("label-picker").hidden, false, "Add label… did not open the picker");
    assert.equal(pickerRows(page)[0].getAttribute("data-value"), "pinned", "Pinned is not the built-in first row");
    await typeAndEnter(page, "#Perf");
    const write = page.labelCalls.find((call) => call.method === "PUT");
    assert.ok(write, "no label write was sent");
    assert.equal(write.id, id, "the write named another message");
    assert.deepStrictEqual(write.body, { name: "#perf" }, "the write's body is not the page's exact request");
    assert.deepStrictEqual(chipTexts(channelRow(page, id)), ["#perf"], "the row does not show the added label");
    assert.equal(page.el("label-picker").hidden, false, "the picker closed after an add (it stays open for more)");
    assert.equal(page.el("label-picker-input").value, "", "the field kept the added name");
    // The label on the message, with its ×: the menu added it, so the menu may take it off.
    const current = page.el("label-picker-current").children;
    assert.equal(current.length, 1);
    const remove = current[0].descendants().find((node) => node.className === "label-picker-remove");
    assert.ok(remove, "a menu-added label has no ×");
    await remove.click();
    await page.settle();
    await page.settle();
    const removal = page.labelCalls.find((call) => call.method === "DELETE");
    assert.ok(removal, "× sent no removal");
    assert.deepStrictEqual(removal.body, { name: "#perf" });
    assert.deepStrictEqual(chipTexts(channelRow(page, id)), [], "the removed label still shows");
  });
}

test("`#2279`: a plain label goes on by name, its chip says no #, and the chip searches the explicit quoted form", async () => {
  const { page, messages } = await labelledPage();
  const id = String(messages[0].id);
  await openPicker(page, channelRow(page, id));
  await page.el("label-picker-input").setValue("Merge  Queue");
  const offered = pickerRows(page).find((li) => li.hasClass("label-picker-new"));
  assert.ok(offered, "no new name is offered");
  assert.equal(offered.children[1].textContent, "Add new label merge queue",
    "the new plain name is not offered as it will be kept");
  await typeAndEnter(page, "Merge  Queue");
  assert.deepStrictEqual(page.labelCalls.find((call) => call.method === "PUT").body, { name: "merge queue" });
  const chip = channelRow(page, id).descendants().find((node) => node.hasClass && node.hasClass("label-chip"));
  assert.equal(chip.textContent, "merge queue");
  assert.ok(chip.hasClass("label-chip-plain"));
  await page.el("label-picker-close").click();
  await page.settle();
  await chip.dispatch("click", { stopPropagation: () => {} });
  await page.settle();
  assert.equal(page.el("search-field").value, 'label:"merge queue"', "a plain chip searched the bare form");
  assert.deepStrictEqual(unfiltered(page, "discord-log").map((li) => li.getAttribute("data-id")), [id]);
});

test("`#2279`: a label the message text carries has no × and the picker says why it stays", async () => {
  const messages = backlog(2);
  messages[0].labels = ["kvm"];
  const { page } = await labelledPage({ messages });
  const id = String(messages[0].id);
  await openPicker(page, channelRow(page, id));
  const current = page.el("label-picker-current").children;
  assert.equal(current.length, 1);
  assert.equal(current[0].descendants().find((node) => node.className === "label-picker-remove"), undefined,
    "a text-derived label offers ×");
  assert.match(page.el("label-picker-note").textContent, /#kvm is in the message text, so it stays/);
  // Tapping its ✓ row says the same, and sends nothing.
  await pickerRowNamed(page, "#kvm").click();
  await page.settle();
  assert.equal(page.labelCalls.filter((call) => call.method !== "GET").length, 0, "a text label was written");
  assert.match(page.el("label-picker-note").textContent, /stays while the text has it/);
});

test("`#2279`: the picker's Pinned row toggles the message's pin, pin and labels in one place", async () => {
  const { page, messages } = await labelledPage();
  const id = String(messages[2].id);
  await openPicker(page, channelRow(page, id));
  await pickerRowNamed(page, "pinned").click();
  await page.settle();
  await page.settle();
  const pinWrite = page.pinCalls.find((call) => call.method === "PUT");
  assert.ok(pinWrite, "the Pinned row did not pin");
  assert.equal(String(pinWrite.id), id);
});

test("`#2279`: another device's label arrives as a `labels` event and shows with no reload", async () => {
  const { page, messages } = await labelledPage();
  const id = String(messages[1].id);
  const reads = channelReads(page).length;
  page.stream().push(`event: labels\ndata: ${JSON.stringify({
    channel_id: page.channels[0].id, message_id: id, labels: ["kvm"], plain_labels: ["merge queue"],
    manual_labels: ["#kvm", "merge queue"], derived: true, revision: 7,
  })}\n\n`);
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(chipTexts(channelRow(page, id)), ["#kvm", "merge queue"], "the event's labels are not on the row");
  assert.equal(channelReads(page).length, reads, "the event cost a channel re-read");
  // An older state for the same message changes nothing.
  page.stream().push(`event: labels\ndata: ${JSON.stringify({
    channel_id: page.channels[0].id, message_id: id, labels: [], plain_labels: [], manual_labels: [],
    derived: true, revision: 6,
  })}\n\n`);
  await page.settle();
  assert.deepStrictEqual(chipTexts(channelRow(page, id)), ["#kvm", "merge queue"], "a stale event undid a newer one");
});

test("`#2279`: a label added or removed while the page missed the event is caught up by the next read's revision", async () => {
  const { page, messages } = await labelledPage();
  const id = String(messages[0].id);
  // Another device labels the message while this page is not listening: only the server knows.
  page.menuLabels.set(id, ["#later"]);
  page.labelsRevision = 4;
  page.messages = page.messages.map((m) => (String(m.id) === id ? { ...m, labels: ["later"], manual_labels: ["#later"] } : m));
  await reReadChannel(page);
  await page.settle();
  assert.ok(page.labelCalls.some((call) => call.method === "GET"), "the moved revision did not read the channel's list");
  assert.deepStrictEqual(chipTexts(channelRow(page, id)), ["#later"], "the missed label did not show");
  // ...and taken off again elsewhere: the complete list no longer names it, so it goes.
  page.menuLabels.set(id, []);
  page.labelsRevision = 5;
  page.messages = page.messages.map((m) => {
    if (String(m.id) !== id) return m;
    const next = { ...m };
    delete next.labels;
    delete next.manual_labels;
    return next;
  });
  await reReadChannel(page);
  await page.settle();
  assert.deepStrictEqual(chipTexts(channelRow(page, id)), [], "a label removed elsewhere still shows");
});

test("`#2279`: the phone's long-press sheet offers Label…, which opens the same picker", async () => {
  const { page, messages } = await labelledPage({ messages: [message({ id: "300", content: "hold me" })] });
  const row = channelRow(page, "300");
  await hold(page, row);
  const sheet = row.descendants().find((node) => node.className === "msg-details");
  assert.ok(sheet, "the long press did not open the details sheet");
  const label = sheet.descendants().find((node) => node.hasClass && node.hasClass("msg-details-label"));
  assert.ok(label, "the details sheet has no Label…");
  assert.equal(label.textContent, "Label…");
  await label.click();
  await page.settle();
  assert.equal(page.el("label-picker").hidden, false, "Label… did not open the picker");
  assert.equal(page.el("label-picker-input").getAttribute("aria-disabled"), "false", "online, the field looked inactive");
  void messages;
});

test("`#2279`: offline, the picker writes nothing and says so, as Pin does", async () => {
  const { page, messages } = await labelledPage();
  const id = String(messages[0].id);
  await page.setOnline(false);
  await openPicker(page, channelRow(page, id));
  assert.match(page.el("label-picker-note").textContent, /^Offline — labels can be added or removed once the connection is back\./);
  assert.equal(page.el("label-picker-input").getAttribute("aria-disabled"), "true", "offline, the field still invited typing");
  await typeAndEnter(page, "#perf");
  assert.equal(page.labelCalls.filter((call) => call.method !== "GET").length, 0, "a label was sent while offline");
  assert.deepStrictEqual(chipTexts(channelRow(page, id)), [], "an offline label was shown as made");
  assert.equal(page.el("label-picker-note").textContent, "Offline — nothing was added. Try again once the connection is back.");
  await page.setOnline(true);
  await page.settle();
  assert.equal(page.labelCalls.filter((call) => call.method !== "GET").length, 0, "the offline label was queued and sent later");
});
