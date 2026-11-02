// The Settings editors' instance and lifetime boundaries, held without the legacy page.
// Page suites retain the controls' behavior; these exercise independent owners and delayed replies
// through their injected capabilities (#624 voice-page-modularity).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const { mountAgentSettings, mountAgentPrefix, mountOwnReadApply } = await moduleForTests("src/view/settings/index.ts");
const { OWN_READ_APPLY_ARM_MS } = await moduleForTests("src/view/settings/ownread.ts");

/** A request whose completion the test orders against another request or an edit. */
function delayed() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

/** One page's nodes, requests and clock, with no browser globals shared with another page. */
function editorPage() {
  const nodes = new Map();
  const timers = new Map();
  const requests = [];
  const statuses = [];
  const reloads = [];
  let nextTimer = 0;
  let lookupCount = 0;
  let answer = async () => { throw new Error("unexpected request"); };
  const deps = {
    lookup(id) {
      lookupCount += 1;
      if (!nodes.has(id)) nodes.set(id, { value: "", checked: false, disabled: false, hidden: false, textContent: "", className: "" });
      return nodes.get(id);
    },
    decoded(type, path, options) {
      requests.push({ type, path, options });
      return answer(path, options);
    },
    request(path, options) {
      requests.push({ path, options });
      return answer(path, options);
    },
    writable: () => true,
    showError: (text) => statuses.push(text),
    setStatus: (text) => statuses.push(text),
    addDetail: (text) => statuses.push(text),
    setTimeout(run, ms) {
      const id = ++nextTimer;
      timers.set(id, { run, ms });
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
    clearArmedMs: () => 4000,
    selectedChannel: () => "one",
    reloadChannel: async (options) => { reloads.push(options); },
    guard: (run) => () => { void run(); },
  };
  return {
    deps, nodes, timers, requests, statuses, reloads,
    lookups: () => lookupCount,
    answer: (run) => { answer = run; },
    expire() {
      const pending = [...timers.values()];
      timers.clear();
      for (const { run } of pending) run();
    },
  };
}

const promptView = (fields = {}) => ({
  instructions: "Keep it brief.", system_prompt: "My prompt", delivery: "page", changeable: true,
  max_instructions_chars: 4000, max_system_prompt_chars: 20000, prompt_version: 2,
  system_prompt_edited: true, edited_from_version: 1, ...fields,
});
const prefixView = (prefix) => ({ prefix, default_prefix: "[default] ", custom: true, changeable: true, max_chars: 100 });

test("`#624 voice-page-modularity`: editor construction is inert, instances share no settings, and switch readers cannot write the owner", () => {
  const a = editorPage();
  const b = editorPage();
  const first = mountAgentSettings(a.deps);
  const second = mountAgentSettings(b.deps);
  const firstPrefix = mountAgentPrefix(a.deps);
  const secondPrefix = mountAgentPrefix(b.deps);
  assert.equal(a.lookups(), 0);
  assert.equal(b.lookups(), 0);
  assert.equal(a.requests.length + b.requests.length, 0);
  assert.equal(a.timers.size + b.timers.size, 0);

  first.applySendSetting({ ask_before_send: false, changeable: true });
  second.applySendSetting({ ask_before_send: true, changeable: false });
  const held = first.sendSetting();
  held.ask_before_send = true;
  held.changeable = false;
  assert.deepStrictEqual(first.sendSetting(), { ask_before_send: false, changeable: true });
  assert.deepStrictEqual(second.sendSetting(), { ask_before_send: true, changeable: false });

  firstPrefix.apply(prefixView("[first] "));
  secondPrefix.apply(prefixView("[second] "));
  firstPrefix.apply(prefixView("[changed] "));
  assert.equal(a.nodes.get("voice-post-prefix").value, "[changed] ");
  assert.equal(b.nodes.get("voice-post-prefix").value, "[second] ");
});

test("`#624 voice-page-modularity`: a prefix owner's older read cannot overwrite its newer read or a write begun after it", async () => {
  const page = editorPage();
  const owner = mountAgentPrefix(page.deps);
  owner.apply(prefixView("[initial] "));
  const older = delayed();
  page.answer(() => older.promise);
  const oldRead = owner.refresh();
  const newer = delayed();
  page.answer(() => newer.promise);
  const newRead = owner.refresh();
  newer.resolve(prefixView("[newer] "));
  await newRead;
  older.resolve(prefixView("[older] "));
  await oldRead;
  assert.equal(page.nodes.get("voice-post-prefix").value, "[newer] ");

  const beforeWrite = delayed();
  page.answer(() => beforeWrite.promise);
  const read = owner.refresh();
  page.nodes.get("voice-post-prefix").value = "[typed] ";
  const saving = delayed();
  page.answer(() => saving.promise);
  const save = owner.save(false);
  beforeWrite.resolve(prefixView("[stale] "));
  await read;
  assert.equal(page.nodes.get("voice-post-prefix").value, "[typed] ");
  assert.equal(page.nodes.get("save-voice-post-prefix").disabled, true);
  saving.resolve(prefixView("[typed] "));
  await save;
  assert.equal(page.nodes.get("voice-post-prefix").value, "[typed] ");
  assert.match(page.nodes.get("voice-post-prefix-state").textContent, /^Saved\./);
});

test("`#624 voice-page-modularity`: disposing a prefix owner drops its pending read without touching another editor", async () => {
  const a = editorPage();
  const b = editorPage();
  const first = mountAgentPrefix(a.deps);
  const second = mountAgentPrefix(b.deps);
  first.apply(prefixView("[first] "));
  second.apply(prefixView("[second] "));
  const pending = delayed();
  a.answer(() => pending.promise);
  const read = first.refresh();
  first.dispose();
  const before = a.lookups();
  pending.resolve(prefixView("[too late] "));
  await read;
  assert.equal(a.lookups(), before, "a retired owner drew a pending answer");
  assert.equal(a.nodes.get("voice-post-prefix").value, "[first] ");
  assert.equal(b.nodes.get("voice-post-prefix").value, "[second] ");
});

test("`#624 voice-page-modularity`: disposing the prompt editor clears only its Restore timer, without drawing", async () => {
  const a = editorPage();
  const b = editorPage();
  const first = mountAgentSettings(a.deps);
  const second = mountAgentSettings(b.deps);
  first.applyPrompt(promptView());
  second.applyPrompt(promptView({ system_prompt: "Another prompt" }));
  await first.restoreSystemPrompt();
  await second.restoreSystemPrompt();
  assert.equal(a.timers.size, 1);
  assert.equal(b.timers.size, 1);
  const before = a.lookups();
  first.dispose();
  assert.equal(a.timers.size, 0);
  assert.equal(a.lookups(), before);
  assert.equal(b.timers.size, 1, "disposing one instance cancelled another's timer");
  b.expire();
  assert.equal(b.nodes.get("restore-voice-system-prompt").textContent, "Restore the shipped prompt");
  assert.equal(a.requests.length + b.requests.length, 0, "arming or retiring Restore saved a prompt");
});

test("`#624 voice-page-modularity`: Apply's receipt is instance-owned and a failed Undo resumes after the batches already restored", async () => {
  const a = editorPage();
  const b = editorPage();
  const first = mountOwnReadApply(a.deps);
  const second = mountOwnReadApply(b.deps);
  assert.equal(a.lookups() + b.lookups(), 0);
  a.answer(async () => ({ changes: [{ channel: { id: "one" }, messages: Array.from({ length: 1002 }, (_, i) => String(i)) }] }));
  await first.apply();
  assert.equal(a.requests.length, 0);
  assert.equal([...a.timers.values()][0].ms, OWN_READ_APPLY_ARM_MS);
  assert.equal(b.timers.size, 0);
  await first.apply();
  assert.equal(a.timers.size, 0);
  assert.equal(a.reloads.length, 1);

  let fail = true;
  a.answer(async (_path, { body }) => {
    if (body.messages.length === 2 && fail) throw new Error("the last batch was refused");
    return null;
  });
  await assert.rejects(first.undo(), /last batch was refused/);
  assert.equal(a.nodes.get("own-read-undo").hidden, false);
  assert.match(a.nodes.get("own-read-state").textContent, /Put 1000 back/);
  assert.deepStrictEqual(a.requests.slice(1).map(({ options }) => options.body.messages.length), [1000, 2]);
  fail = false;
  await first.undo();
  assert.deepStrictEqual(a.requests.slice(1).map(({ options }) => options.body.messages), [
    Array.from({ length: 1000 }, (_, i) => String(i)), ["1000", "1001"], ["1000", "1001"],
  ], "the retry repeated a completed batch or lost the unfinished receipt");
  assert.equal(a.reloads.length, 2);
  await second.undo();
  assert.equal(b.requests.length, 0, "another instance could undo the first's receipt");
  await second.apply();
  second.dispose();
  assert.equal(b.timers.size, 0);
});
