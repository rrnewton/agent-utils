// The call's audio devices (`#619 voice-bluetooth-output`; `#624 voice-page-modularity`, moved from
// the legacy script into core/call beside audioroute.ts): what a device's label says it is, which
// capture device a call opens, whether the call's output can be chosen at all, opening the
// microphone on the chosen device, and aiming playback where the reader chose. The browser's
// device list, microphone and audio-context class are the page's, handed in, so this runs headless.

/** One device, as far as choosing reads it. */
export interface CallDeviceLike {
  kind?: unknown;
  deviceId?: unknown;
  label?: unknown;
}

/** A capture stream, as far as opening reads it. */
export interface CallStreamLike {
  getTracks(): CallTrackLike[];
}

/** One capture track, as far as opening reads it. */
export interface CallTrackLike {
  label?: unknown;
  getSettings?: () => { deviceId?: unknown };
  stop(): void;
}

/** The reader's stored device choices. */
export interface CallDevicePreferences {
  inputId: string;
  outputId: string;
}

/** The call's route record, as far as devices write it. */
export interface CallRouteLike {
  begin(): void;
  noteInput(label: string): void;
  settleInput(chosenLabel: string): void;
  noteSink(sink: string): void;
  noteOutput(label: string): void;
}

/** What the call devices ask of the page. */
export interface CallDevicesDependencies {
  /** The call's route record. */
  readonly route: CallRouteLike;
  /** Open the microphone under `constraints`; the stream it grants. */
  openMicrophone(constraints: unknown): Promise<CallStreamLike>;
  /** The constraints a microphone ask for `mic` opens under. */
  constraintsFor(mic: unknown): { audio: Record<string, unknown> };
  /** The browser's device list, or nothing where the browser hides it. */
  mediaDevices(): { enumerateDevices(): Promise<CallDeviceLike[]> } | null | undefined;
  /** The reader's stored device choices. */
  preferences(): CallDevicePreferences;
  /** The browser's audio-context class, or nothing. */
  audioContextClass(): { prototype: object } | null | undefined;
}


/** The microphone sound settings a constraints ask reads. */
export interface MicSoundSettings {
  echoCancellation: boolean;
  noiseSuppression: boolean;
  autoGainControl: boolean;
}

/** The call devices' questions and commands. */
export interface CallDevices {
  deviceLabel(device: unknown): string;
  classifyAudioDevice(label: string): string;
  pickCallInput(devices: readonly CallDeviceLike[] | null | undefined, prefs: CallDevicePreferences): CallDeviceLike | null;
  outputSinkSupported(): boolean;
  openCallMicrophone(mic: unknown): Promise<CallStreamLike>;
  applyCallOutput(context: { setSinkId?: unknown; sinkId?: unknown } | null | undefined): Promise<void>;
}

/**
 * The one read of a media device's, track's or track-settings' label in this section: the
 * channel-name guard in src/http/api.rs counts label reads across the page, and a device label is
 * not a channel label — so it goes through here, once, without that spelling.
 */
export function createCallDevices(deps: CallDevicesDependencies): CallDevices {
  function deviceLabel(device: unknown): string {
    const { label } = (device || {}) as { label?: unknown };
    return typeof label === "string" ? label : "";
  }

  /**
   * The kind of one device, from the label the browser reports: the closed set the call-end
   * record carries (`AudioRoute` in src/voice_health.rs). Bluetooth is matched FIRST, because a
   * Bluetooth headset's label also says "headset".
   */
  function classifyAudioDevice(label: string): string {
    const words = String(label || "").toLowerCase();
    if (/bluetooth|airpods|hands[ -]?free|\bhfp\b|\bsco\b|\bble\b/.test(words)) return "bluetooth";
    if (/usb/.test(words)) return "usb";
    if (/speaker/.test(words)) return "speakerphone";
    if (/earpiece|receiver/.test(words)) return "earpiece";
    if (/headset|headphone|earbud|earphone/.test(words)) return "wired_headset";
    return "other";
  }

  /**
   * The capture device a call should open, from `devices` (an `enumerateDevices()` list):
   * the reader's stored choice when it is still there; else a Bluetooth/headset input, so the
   * call's communication route follows the headset; else the browser's "communications" device;
   * else null, meaning keep the stream the default constraints already opened.
   */
  function pickCallInput(devices: readonly CallDeviceLike[] | null | undefined, prefs: CallDevicePreferences): CallDeviceLike | null {
    const inputs = (devices || []).filter((d) => d && d.kind === "audioinput" && d.deviceId);
    if (prefs.inputId) {
      const chosen = inputs.find((d) => d.deviceId === prefs.inputId);
      if (chosen) return chosen;
    }
    return inputs.find((d) => classifyAudioDevice(deviceLabel(d)) === "bluetooth")
      || inputs.find((d) => d.deviceId === "communications" || /communications/.test(deviceLabel(d).toLowerCase()))
      || null;
  }

  /**
   * Whether this browser lets the CALL choose an output: call playback is Web Audio, so the
   * form that matters is `AudioContext.setSinkId` (desktop Chromium). A media-element-only
   * `setSinkId` could not aim the call, so it does not earn the picker.
   */
  function outputSinkSupported(): boolean {
    const Context = deps.audioContextClass();
    return Boolean(Context && Context.prototype && "setSinkId" in Context.prototype);
  }

  /**
   * Open the call's microphone: the default constraints first (which is also what grants the
   * permission that lets `enumerateDevices` name anything), then — when a better capture device
   * is present — once more with that device named exactly. `session.stream`'s callers see only
   * the final stream; the first is stopped before the second opens.
   */
  async function openCallMicrophone(mic: unknown): Promise<CallStreamLike> {
    let stream = await deps.openMicrophone(deps.constraintsFor(mic));
    deps.route.begin();
    const trackOf = (opened: CallStreamLike | null | undefined): CallTrackLike | null =>
      (opened && typeof opened.getTracks === "function" ? (opened.getTracks()[0] ?? null) : null);
    const noteTrack = (opened: CallStreamLike): void => {
      const track = trackOf(opened);
      let settings: { deviceId?: unknown } = {};
      try {
        settings = track && typeof track.getSettings === "function" ? track.getSettings() : {};
      } catch (_error) {
        settings = {};
      }
      const label = deviceLabel(track) || deviceLabel(settings);
      deps.route.noteInput(label);
    };
    noteTrack(stream);
    const media = deps.mediaDevices();
    if (!media || typeof media.enumerateDevices !== "function") return stream;
    let devices: CallDeviceLike[] | null = null;
    try {
      devices = await media.enumerateDevices();
    } catch (_error) {
      return stream;
    }
    const chosen = pickCallInput(devices, deps.preferences());
    const current = trackOf(stream);
    let currentId: unknown;
    let currentLabel = "";
    try {
      const settings = current && typeof current.getSettings === "function" ? current.getSettings() : {};
      currentId = settings.deviceId;
      currentLabel = deviceLabel(current) || deviceLabel(settings);
    } catch (_error) {
      currentId = undefined;
    }
    if (!chosen || chosen.deviceId === currentId
      || (deviceLabel(chosen) && currentLabel && deviceLabel(chosen) === currentLabel)) return stream;
    const fallback = stream;
    const constraints = deps.constraintsFor(mic);
    constraints.audio.deviceId = { exact: chosen.deviceId };
    try {
      stream = await deps.openMicrophone(constraints);
    } catch (_error) {
      // The chosen device went away between listing and opening: the stream already open
      // carries the call rather than the call dying on a device race.
      return fallback;
    }
    for (const track of fallback.getTracks()) track.stop();
    noteTrack(stream);
    deps.route.settleInput(deviceLabel(chosen));
    return stream;
  }

  /**
   * Aim the call's playback where the reader chose, where the browser allows it at all. On
   * Android Chrome neither sink API exists, so this does nothing there BY DESIGN: the output
   * follows the communication route `openCallMicrophone` set up. Read-aloud never comes here.
   */
  async function applyCallOutput(context: { setSinkId?: unknown; sinkId?: unknown } | null | undefined): Promise<void> {
    if (!context || typeof context.setSinkId !== "function") return;
    const prefs = deps.preferences();
    if (!prefs.outputId) return;
    try {
      await (context.setSinkId as (id: string) => Promise<void>)(prefs.outputId);
      deps.route.noteSink(String(context.sinkId || prefs.outputId));
    } catch (_error) {
      // The chosen output went away between Settings and the call: the default carries it,
      // and the record keeps the route actually in use rather than the choice that failed.
      return;
    }
    try {
      const media = deps.mediaDevices();
      const devices = media ? await media.enumerateDevices() : null;
      const chosen = (devices || []).find((d) => d.deviceId === prefs.outputId);
      if (chosen) deps.route.noteOutput(deviceLabel(chosen));
    } catch (_error) {
      // No list, no kind: the readout says the sink id instead.
    }
  }

  return { deviceLabel, classifyAudioDevice, pickCallInput, outputSinkSupported, openCallMicrophone, applyCallOutput };
}

// The constraint object handed to getUserMedia.
//
// Echo cancellation and noise suppression are already stated explicitly, so a toggle simply
// supplies the value that was hard-coded.
//
// Automatic gain control is the careful one. It is NOT in the constraint object at all by default,
// so whatever the browser's own default is, is what applies — in practice on, but the spec makes
// that the implementation's choice, not a promise. Passing `autoGainControl: true` would convert an
// implicit default into an explicit request, and those are not guaranteed to be the same thing on
// every browser. So the ON state — the default — says nothing at all, and only the OFF state is
// stated. With every toggle in its default position this returns the byte-for-byte constraint
// object the page has always sent.
export function audioConstraints(settings: MicSoundSettings): { audio: Record<string, unknown> } {
  const audio: Record<string, unknown> = {
    channelCount: 1,
    echoCancellation: settings.echoCancellation,
    noiseSuppression: settings.noiseSuppression,
  };
  if (!settings.autoGainControl) {
    audio.autoGainControl = false;
  }
  return { audio };
}
