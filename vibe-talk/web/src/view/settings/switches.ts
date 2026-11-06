// Settings' switches and choices, each restored as kept before it is heard (`#619 voice-bluetooth-output`, `#58
// control-bar`, `#481 slack-app-deep-link`, `#44 live-push` and `#126 read-new-selector`, `#46 conversation-replay`;
// `#624 voice-page-modularity`, moved from the legacy script's wiring into view/settings for the plan's step 17): the
// microphone's checkboxes, the audio devices, where the bar sits, where Slack links open, the read-new button and
// select, and resuming. The controls carry the live truth, so they are what is restored; what each choice does is its
// own owner's. Mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";
import { MIC_TOGGLES } from "./microphone.ts";
import type { MicrophoneSettings } from "./microphone.ts";

export interface SettingsSwitchesDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** The microphone's settings as kept, and a checkbox changed. */
  storedMicSettings(): MicrophoneSettings;
  micSettingsChanged(): unknown;
  /** `#619 voice-bluetooth-output`. The device selects filled again, one changed, and the route said. */
  refreshAudioDevices(): unknown;
  audioDevicesChanged(): unknown;
  renderAudioRoute(): unknown;
  /** The browser's media devices, which say when one comes or goes, where it has them. */
  mediaDevices(): EventTarget | null | undefined;
  /** `#58 control-bar`. Where the reader last put the bar, and a new place chosen. */
  storedBarPlacement(): string;
  barPlacementChanged(): unknown;
  /** `#481 slack-app-deep-link`. Where this device opens Slack links, and a new choice. */
  storedSlackOpen(): string;
  slackOpenChanged(): unknown;
  /** `#44 live-push`, `#126 read-new-selector`. The bar's button steps to the next mode; the select names one. */
  cycleReadNew(): unknown;
  chooseReadNew(mode: string): unknown;
  /** Say the mode and the live state as they are now. */
  renderReadNew(): unknown;
  renderLiveState(): unknown;
  /** `#46 conversation-replay`. Whether resuming is wanted, as kept, and the switch changed. */
  resumeWanted(): boolean;
  resumeToggled(): unknown;
}

/** The listeners, mounted. */
export interface SettingsSwitches {
  /** Take every listener down again. */
  unmount(): void;
}

/** Restore Settings' switches and choices and mount their listeners, in the page's order. */
export function mountSettingsSwitches(deps: SettingsSwitchesDependencies): SettingsSwitches {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  // The checkboxes are the live truth `start()` reads, so they are what gets restored on load.
  const restoredMicSettings = deps.storedMicSettings();
  for (const [id, key] of MIC_TOGGLES) {
    (el(id) as HTMLInputElement).checked = restoredMicSettings[key];
    listen<Event>(el(id), "change", () => deps.micSettingsChanged());
  }
  // `#619 voice-bluetooth-output`. The device selects carry the live truth, as the toggles do.
  deps.refreshAudioDevices();
  listen<Event>(el("audio-input-device"), "change", () => deps.audioDevicesChanged());
  listen<Event>(el("audio-output-device"), "change", () => deps.audioDevicesChanged());
  listen<Event>(el("audio-route-toggle"), "change", () => deps.renderAudioRoute());
  const media = deps.mediaDevices();
  if (media && typeof media.addEventListener === "function") {
    listen<Event>(media, "devicechange", () => {
      deps.refreshAudioDevices();
      deps.renderAudioRoute();
    });
  }
  // `#58 control-bar`. Where the reader last put the bar, restored before the first screen is drawn — the select
  // carries the live truth, exactly as the microphone checkboxes do.
  (el("bar-placement") as HTMLSelectElement).value = deps.storedBarPlacement();
  listen<Event>(el("bar-placement"), "change", () => deps.barPlacementChanged());
  // `#481 slack-app-deep-link`. Where this device opens Slack links, as the bar's placement is.
  (el("slack-open-in") as HTMLSelectElement).value = deps.storedSlackOpen();
  listen<Event>(el("slack-open-in"), "change", () => deps.slackOpenChanged());
  // `#44 live-push`, extended by `#126 read-new-selector`. TWO controls over one value: the button on the bar, where
  // the reader is when a message arrives, and the select in Settings, where there is room to say what the three modes
  // mean. Both go through `chooseReadNew`, which writes the key and redraws the other one. The bar button CYCLES rather
  // than choosing, because it has room for a word and not for a list.
  listen<Event>(el("read-new"), "click", () => deps.cycleReadNew());
  listen<Event>(el("relay-to-agent"), "change", () => deps.chooseReadNew((el("relay-to-agent") as HTMLSelectElement).value));
  // Stated once at load rather than left to the first use: the mode comes out of storage, so the markup cannot be
  // the one that says what it is.
  deps.renderReadNew();
  deps.renderLiveState();
  // `#46 conversation-replay`. Off unless asked for, and restored the same way: a control that re-sends earlier
  // conversation content to a paid vendor on every call must not come back on by itself.
  (el("resume-toggle") as HTMLInputElement).checked = deps.resumeWanted();
  listen<Event>(el("resume-toggle"), "change", () => deps.resumeToggled());
  return { unmount };
}
