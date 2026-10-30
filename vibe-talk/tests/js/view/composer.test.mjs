// Tests for web/src/view/composer/ on its own: the module built as the page build builds it and
// held directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over
// the same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The message boxes' keys and growth (web/src/view/composer). */
const VIEW_COMPOSER = await moduleForTests("src/view/composer/index.ts");

test("`#624 voice-page-modularity`: what a key does in a message box, and a paste's own new line, from the module alone", () => {
  const { createComposerKeys, PASTE_ENTER_MS } = VIEW_COMPOSER;
  let now = 1000;
  let enter = true;
  const keys = createComposerKeys({ now: () => now, enterSends: () => enter });
  const box = {};
  let prevented = 0;
  const key = (over = {}) => ({ key: "Enter", preventDefault: () => { prevented += 1; }, ...over });
  const send = () => "sent";
  assert.equal(keys.onKey(key(), send, box), "sent");
  assert.equal(keys.onKey(key({ shiftKey: true }), send, box), undefined, "Shift+Enter sent");
  assert.equal(keys.onKey(key({ isComposing: true }), send, box), undefined, "an Enter accepting a composed word sent");
  assert.equal(keys.onKey(key({ keyCode: 229 }), send, box), undefined);
  assert.equal(keys.onKey(key({ repeat: true }), send, box), undefined, "a held key's repeat sent again");
  assert.equal(keys.onKey(key({ key: "a" }), send, box), undefined);
  enter = false;
  assert.equal(keys.onKey(key(), send, box), undefined, "a bare Enter sent with Enter to send off");
  assert.equal(keys.onKey(key({ metaKey: true }), send, box), "sent", "the chord did not send with the setting off");
  // An Enter within PASTE_ENTER_MS of a paste or a run of text in the same box is that paste's new line.
  enter = true;
  keys.noteInsertion(box, { type: "paste" });
  assert.equal(keys.onKey(key(), send, box), undefined, "a paste's own new line sent it");
  assert.equal(keys.onKey(key(), send, {}), "sent", "a paste in one box held another's Enter");
  now += PASTE_ENTER_MS + 1;
  assert.equal(keys.onKey(key(), send, box), "sent");
  keys.noteInsertion(box, { inputType: "insertText", data: "x" });
  assert.equal(keys.insertedJustNow(box), false, "one typed character counted as a paste");
  keys.noteInsertion(box, { inputType: "insertText", data: "two words" });
  assert.equal(keys.insertedJustNow(box), true);
  keys.noteInsertion(box, { inputType: "insertText", data: "ab", isComposing: true });
  assert.ok(prevented > 0);
});

test("`#624 voice-page-modularity`: the boxes that grow, on stand-in elements, from the module alone", () => {
  const { createComposerGrow, COMPOSER_GROW_BOXES, COMPOSER_GROW_IDS } = VIEW_COMPOSER;
  const made = new Map();
  const element = (id) => {
    if (!made.has(id)) {
      const attributes = new Set();
      made.set(id, { id, checked: false, scrollTop: 0, clientHeight: 400, drawn: true, removed: [],
        setAttribute: (name) => attributes.add(name), removeAttribute: (name) => attributes.delete(name),
        hasAttribute: (name) => attributes.has(name), getClientRects() { return this.drawn ? [{}] : []; },
        getBoundingClientRect: () => ({ top: 0, bottom: 20, height: 20 }),
        style: { removeProperty(name) { made.get(id).removed.push(name); } } });
    }
    return made.get(id);
  };
  const record = { kept: [], followed: [], kept_in_view: [] };
  let focused = null;
  let atFoot = false;
  const grow = createComposerGrow({
    lookup: element, listOf: (field) => element(field.id === "side-compose-text" ? "side-scroll" : "scroll-area"),
    atBottom: () => atFoot, followFoot: (area) => record.followed.push(area.id),
    keepBoxInView: (box, area) => record.kept_in_view.push([box.id, area.id]), activeElement: () => focused,
    bottomSlack: 24, fieldSizing: true, keep: (on) => { if (on === "refuse") throw new Error("refused"); record.kept.push(on); },
  });
  assert.equal(grow.on(), false, "the boxes grew before Grow was ticked");
  grow.render(true);
  assert.equal(element("screen-main").hasAttribute("data-composer-grow"), true);
  assert.deepStrictEqual(COMPOSER_GROW_IDS.map((id) => element(id).checked), [true, true, true, true], "a checkbox did not say the one choice");
  assert.deepStrictEqual(COMPOSER_GROW_BOXES.map((id) => element(id).removed), [["height"], ["height"], ["height"], ["height"]],
    "a dragged height was not let go as the boxes began to grow");
  assert.equal(grow.shown(element("channel-compose-text")), true);
  element("channel-compose-grow").drawn = false;
  assert.equal(grow.shown(element("channel-compose-text")), false, "a box grew where its checkbox is not drawn");
  // A list at its foot is put back there as Grow changes; storage refused still honours the choice.
  atFoot = true;
  grow.apply("refuse", element("compose-grow"));
  assert.deepStrictEqual(record.followed, ["scroll-area", "side-scroll"]);
  // Only the box with the focus is kept in view as it grows.
  grow.render(true);
  grow.keepInView(element("side-compose-text"));
  assert.deepStrictEqual(record.kept_in_view, [], "a box without the focus moved its list");
  focused = element("side-compose-text");
  atFoot = false;
  grow.keepInView(element("side-compose-text"));
  assert.deepStrictEqual(record.kept_in_view, [["side-composer", "side-scroll"]]);
});
