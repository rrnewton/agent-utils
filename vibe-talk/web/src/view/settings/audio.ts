// Settings' audio-device choices and opt-in route readout (`#619 voice-bluetooth-output`;
// `#624 voice-page-modularity`), moved from the legacy script. Opening capture, selecting the call's
// output and its route record remain the voice owner's. The editor reads them only when asked.

import type { StoragePort } from "../../ports/index.ts";

const AUDIO_DEVICES_KEY = "vibe-talk.voice.audio-devices";

export interface AudioDevicePreferences {
  inputId: string;
  outputId: string;
}

/** Browser capabilities and the current call, looked up only when a control needs them. */
export interface AudioDevicesDependencies {
  lookup(id: string): HTMLElement;
  readonly storage: StoragePort;
  mediaDevices(): Pick<MediaDevices, "enumerateDevices"> | null | undefined;
  outputSinkSupported(): boolean;
  deviceLabel(device: unknown): string;
  classifyAudioDevice(label: string): string;
  stream(): MediaStream | null;
  audio(): { readonly sinkId?: string } | null;
  route(): { readonly inputKind: string } | null;
}

export interface AudioDevicesEditor {
  stored(): AudioDevicePreferences;
  preferences(): AudioDevicePreferences;
  refresh(): Promise<void>;
  changed(): void;
  renderRoute(): Promise<void>;
}

/** Construction enumerates no device and reads no preference. */
export function mountAudioDevices(deps: AudioDevicesDependencies): AudioDevicesEditor {
  /** What was stored: `{inputId, outputId}`, each "" for System default. Anything else is ignored. */
  function storedAudioDevicePrefs(): AudioDevicePreferences {
    let stored: Record<string, unknown> | null = null;
    try {
      stored = JSON.parse(deps.storage.getItem(AUDIO_DEVICES_KEY) || "null");
    } catch (_error) {
      stored = null;
    }
    const prefs = { inputId: "", outputId: "" };
    if (stored && typeof stored === "object") {
      if (typeof stored.inputId === "string") prefs.inputId = stored.inputId;
      if (typeof stored.outputId === "string") prefs.outputId = stored.outputId;
    }
    return prefs;
  }

  function persistAudioDevicePrefs(prefs: AudioDevicePreferences): boolean {
    const encoded = JSON.stringify(prefs);
    try {
      deps.storage.setItem(AUDIO_DEVICES_KEY, encoded);
    } catch (_error) {
      return false;
    }
    return deps.storage.getItem(AUDIO_DEVICES_KEY) === encoded;
  }

  /** The Settings selects' word, falling back to storage before they exist. */
  function audioDevicePrefs(): AudioDevicePreferences {
    const prefs = storedAudioDevicePrefs();
    const input = document.getElementById("audio-input-device") as HTMLSelectElement | null;
    const output = document.getElementById("audio-output-device") as HTMLSelectElement | null;
    // A select still showing System default may only not have been filled yet (its refresh is
    // asynchronous): storage, which the change handler keeps current, is the safer word then.
    if (input && input.value) prefs.inputId = input.value;
    if (output && output.value) prefs.outputId = output.value;
    return prefs;
  }

  /** Fill the Settings device selects from what the browser can name right now. */
  async function refreshAudioDeviceSelects(): Promise<void> {
    const input = document.getElementById("audio-input-device") as HTMLSelectElement | null;
    const output = document.getElementById("audio-output-device") as HTMLSelectElement | null;
    const outputField = document.getElementById("audio-output-field");
    if (outputField) outputField.hidden = !deps.outputSinkSupported();
    if (!input && !output) return;
    const prefs = storedAudioDevicePrefs();
    let devices: MediaDeviceInfo[] = [];
    if (deps.mediaDevices() && typeof deps.mediaDevices()!.enumerateDevices === "function") {
      try {
        devices = await deps.mediaDevices()!.enumerateDevices();
      } catch (_error) {
        devices = [];
      }
    }
    const fill = (select: HTMLSelectElement | null, kind: MediaDeviceKind, chosenId: string): void => {
      if (!select) return;
      const listed = devices.filter((d) => d && d.kind === kind);
      select.replaceChildren();
      const system = document.createElement("option");
      system.value = "";
      system.textContent = "System default";
      select.append(system);
      listed.forEach((device, at) => {
        const option = document.createElement("option");
        option.value = device.deviceId;
        option.textContent = deps.deviceLabel(device) || `${kind === "audioinput" ? "Microphone" : "Output"} ${at + 1}`;
        select.append(option);
      });
      if (chosenId && !listed.some((d) => d.deviceId === chosenId)) {
        const gone = document.createElement("option");
        gone.value = chosenId;
        gone.textContent = "Chosen device (not connected now)";
        select.append(gone);
      }
      select.value = chosenId;
    };
    fill(input, "audioinput", prefs.inputId);
    fill(output, "audiooutput", prefs.outputId);
  }

  function audioDevicesChanged(): void {
    const saved = persistAudioDevicePrefs(audioDevicePrefs());
    const when = deps.stream()
      ? "The call in progress keeps the devices it opened with — hang up and start again to apply it."
      : "It applies the next time you start talking.";
    deps.lookup("audio-devices-state").textContent = saved
      ? `Saved. ${when}`
      : `This browser refused to store the choice, so it will be forgotten when you reload. ${when}`;
    renderAudioRouteReadout();
  }

  /**
   * The opt-in readout (`#619`): the chosen devices, and the route the browser REPORTS — the
   * open track's own settings and the context's sink — so one screenshot shows whether Chrome
   * took the call to the headset or left it on the speaker. Labels live here only; the server
   * record carries kinds, never names.
   */
  async function renderAudioRouteReadout(): Promise<void> {
    const box = document.getElementById("audio-route-readout");
    const toggle = document.getElementById("audio-route-toggle") as HTMLInputElement | null;
    if (!box || !toggle) return;
    box.hidden = !toggle.checked;
    if (!toggle.checked) return;
    const prefs = audioDevicePrefs();
    const lines = [];
    let devices: MediaDeviceInfo[] = [];
    if (deps.mediaDevices() && typeof deps.mediaDevices()!.enumerateDevices === "function") {
      try {
        devices = await deps.mediaDevices()!.enumerateDevices();
      } catch (_error) {
        devices = [];
      }
    }
    const nameOf = (id: string, kind: MediaDeviceKind): string => {
      if (!id) return "System default";
      const found = devices.find((d) => d.deviceId === id && d.kind === kind);
      return found ? (deps.deviceLabel(found) || found.deviceId) : `${id} (not connected now)`;
    };
    lines.push(`Microphone choice: ${nameOf(prefs.inputId, "audioinput")}`);
    lines.push(deps.outputSinkSupported()
      ? `Speaker choice: ${nameOf(prefs.outputId, "audiooutput")}`
      : "Speaker choice: this browser cannot choose an output (on Android the call follows the microphone's route)");
    const inputs = devices.filter((d) => d.kind === "audioinput");
    lines.push(`Microphones the browser lists: ${inputs.length
      ? inputs.map((d) => `${deps.deviceLabel(d) || d.deviceId} [${deps.classifyAudioDevice(deps.deviceLabel(d))}]`).join("; ")
      : "none yet — start a call once so the browser may name them"}`);
    if (deps.stream()) {
      const track = deps.stream()!.getTracks()[0];
      const settings = track && typeof track.getSettings === "function" ? track.getSettings() : {};
      lines.push(`This call's microphone: ${deps.deviceLabel(settings) || deps.deviceLabel(track) || settings.deviceId || "browser did not say"} [${deps.route() ? deps.route()!.inputKind : "other"}]`);
    } else {
      lines.push("This call's microphone: no call is open");
    }
    if (deps.audio()) {
      lines.push(`This call's speaker: ${deps.audio()!.sinkId ? `sink ${deps.audio()!.sinkId}` : "the route the microphone chose (browser does not report a speaker)"}`);
    }
    box.textContent = lines.join("\n");
  }

  return { stored: storedAudioDevicePrefs, preferences: audioDevicePrefs,
    refresh: refreshAudioDeviceSelects, changed: audioDevicesChanged, renderRoute: renderAudioRouteReadout };
}
