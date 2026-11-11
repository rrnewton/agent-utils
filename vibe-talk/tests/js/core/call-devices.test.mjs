// Tests for the call's audio devices in web/src/core/call/ on their own: the module built as the
// page build builds it and held directly, with fake device lists, streams and sinks, and no page
// around it (`#624 voice-page-modularity`). The page's behaviour over the same module, on the
// fixture's DOM, is tested in ../page/call.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The call's audio devices (web/src/core/call). */
const CALL = await moduleForTests("src/core/call/index.ts");

/** A device list and ports that say what opening did, in order. */
const harness = (overrides = {}) => {
  const log = [];
  const route = {
    begin: () => log.push("begin"),
    noteInput: (label) => log.push(`input ${label}`),
    settleInput: (label) => log.push(`settle ${label}`),
    noteSink: (sink) => log.push(`sink ${sink}`),
    noteOutput: (label) => log.push(`output ${label}`),
  };
  const stream = (id, label, deviceId) => ({
    getTracks: () => [{ label, getSettings: () => ({ deviceId }), stop: () => log.push(`stop ${id}`) }],
  });
  const devices = CALL.createCallDevices({
    route,
    openMicrophone: async (constraints) => {
      log.push(`open ${JSON.stringify(constraints)}`);
      return overrides.open ? overrides.open(constraints) : stream("first", "Built-in Microphone", "default");
    },
    constraintsFor: () => ({ audio: {} }),
    mediaDevices: () => overrides.media,
    preferences: () => overrides.prefs || { inputId: "", outputId: "" },
    audioContextClass: () => overrides.contextClass,
  });
  return { devices, log, stream };
};

test("`#624 voice-page-modularity`: a label classifies Bluetooth first, and a stored choice beats the headset rule", () => {
  const { devices } = harness();
  assert.equal(devices.deviceLabel({ label: "AirPods Pro" }), "AirPods Pro");
  assert.equal(devices.deviceLabel({}), "");
  assert.equal(devices.deviceLabel(null), "");
  assert.equal(devices.classifyAudioDevice("Bluetooth Headset"), "bluetooth");
  assert.equal(devices.classifyAudioDevice("USB Audio"), "usb");
  assert.equal(devices.classifyAudioDevice("Speakerphone"), "speakerphone");
  assert.equal(devices.classifyAudioDevice("Earpiece"), "earpiece");
  assert.equal(devices.classifyAudioDevice("Wired Headphones"), "wired_headset");
  assert.equal(devices.classifyAudioDevice("Built-in Microphone"), "other");
  const list = [
    { kind: "audioinput", deviceId: "bt", label: "AirPods" },
    { kind: "audioinput", deviceId: "usb", label: "USB Mic" },
    { kind: "audiooutput", deviceId: "spk", label: "Speaker" },
  ];
  assert.equal(devices.pickCallInput(list, { inputId: "usb", outputId: "" }).deviceId, "usb");
  assert.equal(devices.pickCallInput(list, { inputId: "", outputId: "" }).deviceId, "bt");
  assert.equal(devices.pickCallInput([{ kind: "audioinput", deviceId: "communications", label: "" }], { inputId: "", outputId: "" }).deviceId, "communications");
  assert.equal(devices.pickCallInput([], { inputId: "", outputId: "" }), null);
});

test("`#624 voice-page-modularity`: sink support is the audio context's setSinkId, and output aims only with a stored choice", async () => {
  class SinkContext { }
  SinkContext.prototype.setSinkId = async () => {};
  const supported = harness({ contextClass: SinkContext, prefs: { inputId: "", outputId: "desk" },
    media: { enumerateDevices: async () => [{ kind: "audiooutput", deviceId: "desk", label: "USB speaker" }] } });
  assert.equal(supported.devices.outputSinkSupported(), true);
  assert.equal(harness({ contextClass: class { } }).devices.outputSinkSupported(), false);
  const aimed = [];
  await supported.devices.applyCallOutput({ setSinkId: async (id) => { aimed.push(id); }, sinkId: "desk" });
  assert.deepStrictEqual(aimed, ["desk"]);
  assert.deepStrictEqual(supported.log, ["sink desk", "output USB speaker"]);
  const noChoice = harness({ contextClass: SinkContext, media: { enumerateDevices: async () => [] } });
  await noChoice.devices.applyCallOutput({ setSinkId: async (id) => { aimed.push(id); }, sinkId: "" });
  assert.deepStrictEqual(aimed, ["desk"], "an output with no stored choice was aimed");
  await supported.devices.applyCallOutput(null);
  await supported.devices.applyCallOutput({});
});

test("`#624 voice-page-modularity`: the microphone reopens on the chosen device, stops the first stream, and keeps the first when the chosen one is already live", async () => {
  const { devices, log, stream } = harness({
    media: { enumerateDevices: async () => [
      { kind: "audioinput", deviceId: "bt", label: "AirPods" },
      { kind: "audioinput", deviceId: "default", label: "Built-in Microphone" },
    ] },
  });
  let opens = 0;
  const reopening = CALL.createCallDevices({
    route: { begin: () => log.push("begin"), noteInput: (label) => log.push(`input ${label}`),
      settleInput: (label) => log.push(`settle ${label}`), noteSink: () => {}, noteOutput: () => {} },
    openMicrophone: async () => { opens += 1; return opens === 1 ? stream("first", "Built-in Microphone", "default") : stream("second", "AirPods", "bt"); },
    constraintsFor: () => ({ audio: {} }),
    mediaDevices: () => ({ enumerateDevices: async () => [
      { kind: "audioinput", deviceId: "bt", label: "AirPods" },
      { kind: "audioinput", deviceId: "default", label: "Built-in Microphone" },
    ] }),
    preferences: () => ({ inputId: "", outputId: "" }),
    audioContextClass: () => null,
  });
  const final = await reopening.openCallMicrophone({});
  assert.equal(final.getTracks()[0].label, "AirPods");
  assert.deepStrictEqual(log, ["begin", "input Built-in Microphone", "stop first", "input AirPods", "settle AirPods"]);
  assert.equal(devices.outputSinkSupported(), false);
});

test("`#624 voice-page-modularity`: a chosen device already live — by id, or by label alone — keeps the first stream and opens nothing more", async () => {
  const { log } = harness();
  const live = async (firstTrack, enumerated) => {
    let opens = 0;
    const owner = CALL.createCallDevices({
      route: { begin: () => log.push("begin"), noteInput: (label) => log.push(`input ${label}`),
        settleInput: (label) => log.push(`settle ${label}`), noteSink: () => {}, noteOutput: () => {} },
      openMicrophone: async () => { opens += 1; return { getTracks: () => [firstTrack] }; },
      constraintsFor: () => ({ audio: {} }),
      mediaDevices: () => ({ enumerateDevices: async () => enumerated }),
      preferences: () => ({ inputId: "", outputId: "" }),
      audioContextClass: () => null,
    });
    const final = await owner.openCallMicrophone({});
    return { opens, finalTrack: final.getTracks()[0] };
  };
  // Same deviceId as the live track's settings: no second open, nothing stopped, no settle.
  const byIdTrack = { label: "AirPods", getSettings: () => ({ deviceId: "bt" }), stop: () => log.push("stop") };
  const byId = await live(byIdTrack, [{ kind: "audioinput", deviceId: "bt", label: "AirPods" }]);
  assert.equal(byId.opens, 1);
  assert.equal(byId.finalTrack, byIdTrack);
  // Settings name no id, but the live track carries the chosen device's label: likewise kept.
  log.length = 0;
  const byLabelTrack = { label: "AirPods", getSettings: () => ({}), stop: () => log.push("stop") };
  const byLabel = await live(byLabelTrack, [{ kind: "audioinput", deviceId: "bt", label: "AirPods" }]);
  assert.equal(byLabel.opens, 1);
  assert.equal(byLabel.finalTrack, byLabelTrack);
  assert.deepStrictEqual(log, ["begin", "input AirPods"]);
});

test("the microphone constraints state only an OFF gain, byte for byte as ever", () => {
  assert.deepStrictEqual(
    CALL.audioConstraints({ echoCancellation: true, noiseSuppression: true, autoGainControl: true }),
    { audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } },
    "the default position says nothing about gain, as it always did");
  assert.deepStrictEqual(
    CALL.audioConstraints({ echoCancellation: false, noiseSuppression: false, autoGainControl: false }),
    { audio: { channelCount: 1, echoCancellation: false, noiseSuppression: false, autoGainControl: false } });
});
