// Tests for web/src/platform/web/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The browser's implementations of the ports (web/src/platform/web). */
const WEB_PLATFORM = await moduleForTests("src/platform/web/index.ts");

test("`#624 voice-page-modularity`: the browser's socket, audio, microphone and device-voice ports look each global up when used, not when made", async () => {
  const names = ["WebSocket", "Audio", "window", "navigator"];
  const saved = names.map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  const put = (name, value) => Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  try {
    const ports = WEB_PLATFORM.createWebPorts();
    // Installed after the ports were made: a stand-in a test (or a browser walk) puts in late is used.
    put("WebSocket", class { constructor(url) { this.url = url; } });
    put("Audio", class { constructor(url) { this.src = url; } });
    class Prefixed {}
    put("window", { webkitAudioContext: Prefixed });
    const asked = [];
    put("navigator", { mediaDevices: { getUserMedia(constraints) { asked.push([this === globalThis.navigator.mediaDevices, constraints]); return Promise.resolve("stream"); } } });
    assert.equal(ports.socket.open("wss://voice/a").url, "wss://voice/a");
    assert.equal(ports.audio.element("/speech/1").src, "/speech/1");
    assert.equal(ports.audio.contextClass(), Prefixed, "WebKit's prefixed context was not the fallback");
    class Unprefixed {}
    put("window", { AudioContext: Unprefixed, webkitAudioContext: Prefixed });
    assert.equal(ports.audio.contextClass(), Unprefixed);
    put("window", {});
    assert.equal(ports.audio.contextClass(), undefined);
    assert.equal(await ports.microphone.open({ audio: true }), "stream");
    assert.deepStrictEqual(asked, [[true, { audio: true }]], "getUserMedia was not called on mediaDevices, with the constraints given");
    // The device's voice: an engine only where it can list voices, speak and cancel, and utterances exist.
    const engine = { getVoices() {}, speak() {}, cancel() {} };
    class Utterance { constructor(text) { this.text = text; } }
    put("window", { speechSynthesis: engine, SpeechSynthesisUtterance: Utterance });
    assert.equal(ports.speech.engine(), engine);
    assert.equal(ports.speech.utterance("hello").text, "hello");
    put("window", { speechSynthesis: { getVoices() {}, speak() {} }, SpeechSynthesisUtterance: Utterance });
    assert.equal(ports.speech.engine(), null, "an engine that cannot cancel was used");
    put("window", { speechSynthesis: engine });
    assert.equal(ports.speech.engine(), null, "an engine with no utterances was used");
    put("window", {});
    assert.equal(ports.speech.engine(), null);
  } finally {
    for (const [name, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
});
