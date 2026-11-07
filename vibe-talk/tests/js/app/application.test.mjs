// Tests for web/src/app/ on its own: the ordered boot built as the page build builds it and
// held directly, with fake mounts and start acts and no page around it (`#624
// voice-page-modularity`, step 17). Lands as vibe-talk/tests/js/app.test.mjs with Bundle A.
// Run with the other module suites: node --test tests/js/app.test.mjs (after npm ci --prefix contract).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The ordered boot (web/src/app). */
const APP = await moduleForTests("src/app/index.ts");

/** A fake mount/start world: every call recorded in `log`, in the order it happened. */
function fakeWorld() {
  const log = [];
  const mount = (name) => () => {
    log.push(`mount:${name}`);
    return { unmount: () => log.push(`unmount:${name}`) };
  };
  const start = (name) => () => log.push(`start:${name}`);
  return { log, mount, start };
}

test("`#624 voice-page-modularity`: mount and start run in the given order, and nothing is constructed early", () => {
  const { createApplication } = APP;
  const { log, mount, start } = fakeWorld();
  const app = createApplication({
    mounts: [mount("a"), mount("b"), mount("c")],
    starts: [start("token"), start("first-view"), start("deep-link")],
  });
  assert.deepStrictEqual(log, [], "creating the application mounted or started something");
  app.mount();
  assert.deepStrictEqual(log, ["mount:a", "mount:b", "mount:c"]);
  app.start();
  assert.deepStrictEqual(log, ["mount:a", "mount:b", "mount:c", "start:token", "start:first-view", "start:deep-link"]);
  // A second mount or start is a no-op: production calls each exactly once, and a stray
  // second call must not double-listen.
  app.mount();
  app.start();
  assert.deepStrictEqual(log, ["mount:a", "mount:b", "mount:c", "start:token", "start:first-view", "start:deep-link"]);
});

test("`#624 voice-page-modularity`: start before mount throws, and start after dispose runs nothing", () => {
  const { createApplication } = APP;
  const { log, mount, start } = fakeWorld();
  const app = createApplication({ mounts: [mount("a")], starts: [start("s")] });
  assert.throws(() => app.start(), /mounted before it is started/);
  app.mount();
  app.dispose();
  app.start();
  assert.deepStrictEqual(log, ["mount:a", "unmount:a"], "a disposed boot started");
});

test("`#624 voice-page-modularity`: dispose takes mounts down in reverse order, and twice is once", () => {
  const { createApplication } = APP;
  const { log, mount, start } = fakeWorld();
  const app = createApplication({
    mounts: [mount("a"), mount("b"), mount("c")],
    starts: [start("s")],
  });
  app.dispose(); // Before mount: nothing to take down, and not an error.
  app.mount();
  app.start();
  app.dispose();
  app.dispose();
  assert.deepStrictEqual(log, [
    "mount:a", "mount:b", "mount:c", "start:s",
    "unmount:c", "unmount:b", "unmount:a",
  ]);
});

test("`#624 voice-page-modularity`: two applications in one process share nothing", () => {
  const { createApplication } = APP;
  const one = fakeWorld();
  const two = fakeWorld();
  const first = createApplication({ mounts: [one.mount("a")], starts: [one.start("s")] });
  const second = createApplication({ mounts: [two.mount("a")], starts: [two.start("s")] });
  first.mount();
  first.start();
  first.dispose();
  assert.deepStrictEqual(one.log, ["mount:a", "start:s", "unmount:a"]);
  assert.deepStrictEqual(two.log, [], "disposing the first boot touched the second");
  second.mount();
  second.start();
  second.dispose();
  assert.deepStrictEqual(two.log, ["mount:a", "start:s", "unmount:a"]);
});

test("`#624 voice-page-modularity`: a mount that throws unwinds what mounted before it, in reverse", () => {
  const { createApplication } = APP;
  const { log, mount } = fakeWorld();
  const app = createApplication({
    mounts: [mount("a"), () => { throw new Error("boom"); }, mount("c")],
    starts: [],
  });
  assert.throws(() => app.mount(), /boom/);
  assert.deepStrictEqual(log, ["mount:a", "unmount:a"], "a failed boot left a listener behind");
  app.dispose(); // Already disposed by the failed mount: still idempotent.
  assert.deepStrictEqual(log, ["mount:a", "unmount:a"]);
});

test("`#624 voice-page-modularity`: one teardown throwing does not stop the rest, and the error is the first", () => {
  const { createApplication } = APP;
  const log = [];
  const failing = (name, message) => () => ({
    unmount: () => { log.push(`unmount:${name}`); if (message) throw new Error(message); },
  });
  const app = createApplication({ mounts: [failing("a"), failing("b", "b failed"), failing("c")], starts: [] });
  app.mount();
  assert.throws(() => app.dispose(), /b failed/);
  assert.deepStrictEqual(log, ["unmount:c", "unmount:b", "unmount:a"]);
  app.dispose(); // The error does not make a second dispose retry or rethrow.
  assert.deepStrictEqual(log, ["unmount:c", "unmount:b", "unmount:a"]);
});

test("`#624 voice-page-modularity`: tracking mounts in place collects them for a reverse-order dispose, and a late track after dispose comes down at once", () => {
  const { createApplication } = APP;
  const { log, mount, start } = fakeWorld();
  const app = createApplication({ mounts: [], starts: [start("s")] });
  app.track(mount("a")());
  app.track(mount("b")());
  app.start();
  app.dispose();
  app.track(mount("c")());
  assert.deepStrictEqual(log, ["mount:a", "mount:b", "start:s", "unmount:b", "unmount:a", "mount:c", "unmount:c"]);
});
