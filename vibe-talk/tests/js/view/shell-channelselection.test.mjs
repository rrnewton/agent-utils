// Tests for web/src/view/shell/channelselection.ts on its own (`#624 voice-page-modularity`):
// the selection's owner made on first use, the picker as its projection, and a choice the
// picker does not list choosing none. The page over the same factory is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");
const NAVCORE = await moduleForTests("src/core/navigation/index.ts");

function makeSelection() {
  let made = 0;
  const picker = { value: "unset" };
  const selection = SHELL.createChannelSelection({
    createNavigation: () => { made += 1; return NAVCORE.createNavigation(); },
    picker: () => picker,
    channels: () => [{ id: "c1" }, { id: 2 }],
  });
  return { selection, picker, made: () => made };
}

test("the owner is made on first use, once, with nothing chosen", () => {
  const { selection, picker, made } = makeSelection();
  assert.equal(made(), 0, "making the selection owner made no navigation owner");
  assert.equal(selection.selectedChannelId(), "");
  assert.equal(made(), 1);
  assert.equal(selection.selectionOwner(), selection.selectionOwner(), "one owner, made once");
  assert.equal(made(), 1);
  assert.equal(picker.value, "unset", "nothing chosen, nothing projected yet");
});

test("choosing a listed channel projects it on the picker; an unlisted one chooses none", () => {
  const { selection, picker } = makeSelection();
  selection.chooseChannel("c1");
  assert.equal(selection.selectedChannelId(), "c1");
  assert.equal(picker.value, "c1", "the bar's picker shows the selection");
  selection.chooseChannel("2");
  assert.equal(selection.selectedChannelId(), "2", "ids compare as strings, as the picker takes values");
  selection.chooseChannel("nope");
  assert.equal(selection.selectedChannelId(), "", "a channel the picker does not list chooses none");
  assert.equal(picker.value, "");
});
