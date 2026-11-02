// The microphone's Settings controls (`#624 voice-page-modularity`), moved from the legacy script.
// Three knobs on the audio the browser hands us. They are exposed because each one has a failure
// mode the owner has actually hit, and none of them is guessable from the outside: the agent
// answering its own voice on speakerphone, background noise reaching the transcriber, and — the
// awkward one — automatic gain amplifying a silent room until a phone being set down crosses the
// speech threshold and gets transcribed as a word.

import type { StoragePort } from "../../ports/index.ts";

export interface MicrophoneSettings {
  echoCancellation: boolean;
  noiseSuppression: boolean;
  autoGainControl: boolean;
}

export const MIC_SETTINGS_KEY = "vibe-talk.voice.mic";

export const MIC_TOGGLES = [
  ["mic-echo-cancellation", "echoCancellation"],
  ["mic-noise-suppression", "noiseSuppression"],
  ["mic-auto-gain", "autoGainControl"],
] as const;

const DEFAULT_MIC_SETTINGS: MicrophoneSettings = {
  echoCancellation: true, // stated explicitly today.
  noiseSuppression: true, // stated explicitly today.
  autoGainControl: true, // NOT stated today; see `audioConstraints`.
};

export interface MicrophoneSettingsDependencies {
  lookup(id: string): HTMLElement;
  readonly storage: StoragePort;
  streamOpen(): boolean;
}

export interface MicrophoneSettingsEditor {
  read(): MicrophoneSettings;
  stored(): MicrophoneSettings;
  changed(): void;
}

/** The controls are read on demand; construction neither restores nor writes them. */
export function mountMicrophoneSettings(deps: MicrophoneSettingsDependencies): MicrophoneSettingsEditor {
  /** What the checkboxes say right now. The DOM is the live truth; storage only carries it across reloads. */
  function micSettings(): MicrophoneSettings {
    const settings = {} as MicrophoneSettings;
    for (const [id, key] of MIC_TOGGLES) {
      settings[key] = (deps.lookup(id) as HTMLInputElement).checked === true;
    }
    return settings;
  }

  /** What was stored, defaulted per key. A corrupt or partial entry falls back to today's behaviour. */
  function storedMicSettings(): MicrophoneSettings {
    const settings = { ...DEFAULT_MIC_SETTINGS };
    let stored: Record<string, unknown> | null = null;
    try {
      stored = JSON.parse(deps.storage.getItem(MIC_SETTINGS_KEY) || "null");
    } catch (_error) {
      stored = null;
    }
    if (stored && typeof stored === "object") {
      for (const key of (Object.keys(settings) as (keyof MicrophoneSettings)[])) {
        if (typeof stored[key] === "boolean") {
          settings[key] = stored[key];
        }
      }
    }
    return settings;
  }

  function persistMicSettings(settings: MicrophoneSettings): boolean {
    const encoded = JSON.stringify(settings);
    try {
      deps.storage.setItem(MIC_SETTINGS_KEY, encoded);
    } catch (_error) {
      return false;
    }
    // Read it back rather than assuming, exactly as saving the token does: private browsing accepts
    // setItem and stores nothing, and "saved" would then be a lie found out only after a reload.
    return deps.storage.getItem(MIC_SETTINGS_KEY) === encoded;
  }

  // A toggle flipped mid-call does NOT reach the open microphone — the constraints were read when
  // the stream opened. Saying "saved" and stopping there would leave a control that looks like it
  // did something; so the line under the toggles states which call the change lands on.
  function micSettingsChanged(): void {
    const saved = persistMicSettings(micSettings());
    const when = deps.streamOpen()
      ? "The microphone is already open, so this does NOT change the call in progress — hang up " +
        "and start again to apply it."
      : "It applies the next time you start talking.";
    deps.lookup("mic-settings-state").textContent = saved
      ? `Saved. ${when}`
      : "This browser refused to store the setting, so it will be forgotten when you reload " +
        `(private browsing does this). ${when}`;
  }

  return { read: micSettings, stored: storedMicSettings, changed: micSettingsChanged };
}
