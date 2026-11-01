// The call owners can be constructed before the rest of the page is initialized. Their effects
// belong to explicit actions and initialization seams, not construction (#624 voice-page-modularity).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const VIEW_VOICE = await moduleForTests("src/view/voice/index.ts");

for (const name of ["createVoiceCall", "createVoiceTranscript", "createReadAloud"]) {
  test(`\`#624 voice-page-modularity\`: ${name} waits for explicit actions before reading dependencies or starting effects`, () => {
    const attempts = [];
    const refuse = (what) => () => {
      attempts.push(what);
      throw new Error(`construction reached ${what}`);
    };
    const unreadable = (what) => new Proxy({}, {
      get(_target, key) { return refuse(`${what}.${String(key)}`)(); },
    });
    const replacements = [
      [globalThis, "document", unreadable("document")],
      [globalThis, "window", unreadable("window")],
      [globalThis, "setTimeout", refuse("setTimeout")],
      [globalThis, "setInterval", refuse("setInterval")],
      [globalThis, "requestAnimationFrame", refuse("requestAnimationFrame")],
      [globalThis, "fetch", refuse("fetch")],
      [Date, "now", refuse("Date.now")],
      [Math, "random", refuse("Math.random")],
    ];
    const saved = replacements.map(([object, key]) => Object.getOwnPropertyDescriptor(object, key));
    try {
      for (const [object, key, value] of replacements) {
        Object.defineProperty(object, key, { configurable: true, writable: true, value });
      }
      const owner = VIEW_VOICE[name](unreadable("dependencies"));
      for (const key of Object.keys(owner)) {
        const held = owner[key];
        if (typeof held === "function") {
          assert.equal(owner[key], held, `${name}.${key} changed callback identity`);
        }
      }
      // An owner catching and hiding an eager failure is still an owner that ran too early.
      assert.deepEqual(attempts, [], "construction attempted an effect before its explicit action");
    } finally {
      replacements.forEach(([object, key], index) => {
        if (saved[index]) Object.defineProperty(object, key, saved[index]);
        else delete object[key];
      });
    }
  });
}
