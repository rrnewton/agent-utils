// Tests for web/src/view/settings/ on its own: the module built as the page build builds it and held
// directly, handed elements of its own with no page around them (`#624 voice-page-modularity`). The
// whole page's Settings and Help, folded, are tested in ../page/settings.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The Settings and Help screens as the DOM draws them (web/src/view/settings). */
const VIEW_SETTINGS = await moduleForTests("src/view/settings/index.ts");
/** The lists the folds are drawn from (web/src/present/settings). */
const PRESENT_SETTINGS = await moduleForTests("src/present/settings/index.ts");

/** An element with attributes, listeners, a parent and `hidden`: what the folds touch. */
class Node {
  constructor(id, parentNode = null) {
    this.id = id;
    this.parentNode = parentNode;
    this.hidden = false;
    this.open = false;
    this.textContent = "";
    this.attributes = new Map();
    this.listeners = new Map();
    this.scrolled = [];
  }
  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }
  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }
  removeAttribute(name) {
    this.attributes.delete(name);
  }
  addEventListener(type, listener) {
    (this.listeners.get(type) || this.listeners.set(type, []).get(type)).push(listener);
  }
  fire(type) {
    for (const listener of this.listeners.get(type) || []) listener({ type });
  }
  scrollIntoView(how) {
    this.scrolled.push(how);
  }
}

/** Settings with every fold's button and body, each group inside its own section's body. */
function settingsPage() {
  const nodes = new Map();
  const screen = new Node("screen-settings");
  nodes.set("fold-settings-all", new Node("fold-settings-all", screen));
  let section = screen;
  for (const slug of PRESENT_SETTINGS.SETTINGS_FOLDS) {
    const parent = slug.startsWith("settings-") ? screen : section;
    const button = new Node(`fold-${slug}`, parent);
    const body = new Node(`fold-${slug}-body`, parent);
    nodes.set(button.id, button);
    nodes.set(body.id, body);
    if (slug.startsWith("settings-")) section = body;
  }
  let closed = 0;
  const folds = VIEW_SETTINGS.mountSettingsFolds({ lookup: (id) => nodes.get(id), closeDismissed: () => { closed += 1; } });
  return { nodes, folds, closed: () => closed };
}

test("`#624 voice-page-modularity`: Settings mounts folded, a heading opens and folds its group, and Expand all counts only the folds on the page", () => {
  const { nodes, folds, closed } = settingsPage();
  const all = nodes.get("fold-settings-all");
  for (const slug of PRESENT_SETTINGS.SETTINGS_FOLDS) {
    assert.equal(nodes.get(`fold-${slug}`).getAttribute("aria-expanded"), "false");
    assert.equal(nodes.get(`fold-${slug}-body`).getAttribute("hidden"), "until-found");
  }
  assert.equal(all.textContent, "Expand all");
  nodes.get("fold-microphone").fire("click");
  assert.equal(folds.open("microphone"), true);
  assert.equal(nodes.get("fold-microphone-body").getAttribute("hidden"), null);
  assert.deepStrictEqual(nodes.get("fold-microphone").scrolled, [{ block: "nearest" }], "the heading pressed was not kept in view");
  // Folding a fold that holds the pop-out closes it; another fold does not.
  nodes.get("fold-dismissed").fire("click");
  nodes.get("fold-dismissed").fire("click");
  assert.equal(closed(), 1);
  // The browser's find opens a body: the button says so.
  nodes.get("fold-typing-body").fire("beforematch");
  assert.equal(folds.open("typing"), true);
  // A group hidden is not on the page, and Expand all does not wait for it.
  nodes.get("fold-settings-all").fire("click");
  assert.equal(all.textContent, "Fold all");
  folds.set("provider", false);
  assert.equal(all.textContent, "Expand all");
  nodes.get("fold-provider").parentNode.hidden = true;
  assert.equal(folds.shown("provider"), true, "a folded body around it is not hiding it");
  const group = new Node("provider-group", nodes.get("fold-provider").parentNode);
  group.hidden = true;
  nodes.get("fold-provider").parentNode = group;
  assert.equal(folds.shown("provider"), false);
  folds.renderAll();
  assert.equal(all.textContent, "Fold all");
  // Fold all folds every fold, the hidden one's too, and closes the pop-out.
  all.fire("click");
  for (const slug of PRESENT_SETTINGS.SETTINGS_FOLDS) assert.equal(folds.open(slug), false, `Fold all left ${slug} open`);
  assert.equal(closed(), 2);
});

test("`#624 voice-page-modularity`: Help opens at one entry, folds the rest, and its button counts only the entries on the screen", () => {
  const nodes = new Map([["fold-help", new Node("fold-help")]]);
  for (const slug of PRESENT_SETTINGS.helpEntrySlugs()) nodes.set(`help-${slug}`, new Node(`help-${slug}`));
  const help = VIEW_SETTINGS.mountHelpFolds({ lookup: (id) => nodes.get(id), byId: (id) => nodes.get(id) || null });
  const button = nodes.get("fold-help");
  assert.equal(help.entries().length, PRESENT_SETTINGS.HELP_TOPICS.length + 2);
  help.openAt("keyboard");
  assert.deepStrictEqual(help.entries().filter((entry) => entry.open).map((entry) => entry.id), ["help-keyboard"]);
  assert.deepStrictEqual(nodes.get("help-keyboard").scrolled, [{ block: "start" }]);
  assert.equal(button.textContent, "Expand all");
  button.fire("click");
  assert.ok(help.entries().every((entry) => entry.open));
  assert.equal(button.textContent, "Fold all");
  // An entry the reader folds is heard; a hidden one is not counted.
  nodes.get("help-storage").open = false;
  nodes.get("help-storage").fire("toggle");
  assert.equal(button.textContent, "Expand all");
  nodes.get("help-storage").hidden = true;
  help.render();
  assert.equal(button.textContent, "Fold all");
  // No slug: every entry folded, nothing scrolled to.
  help.openAt(null);
  assert.ok(help.entries().every((entry) => !entry.open));
  assert.deepStrictEqual(nodes.get("help-keyboard").scrolled, [{ block: "start" }]);
});
