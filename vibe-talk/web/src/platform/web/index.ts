// The browser's implementation of the core's ports (`#624 voice-page-modularity`). Each reads the
// browser global it stands for at the moment it is used, never once at construction, so whatever
// the page's environment holds then is what is used.

import type {
  AudioPort,
  CancellationPort,
  ClockPort,
  HttpPort,
  HttpRequestInit,
  MicrophonePort,
  SocketPort,
  SpeechSynthesisPort,
  StoragePort,
  TimerHandle,
} from "../../ports/index.ts";

/** The window as an older WebKit gives it: its audio context class under a prefixed name. */
type PrefixedWindow = Window & { readonly webkitAudioContext?: typeof AudioContext };

/** The ports the browser gives the core. */
export interface WebPorts {
  readonly http: HttpPort;
  readonly clock: ClockPort;
  readonly cancellation: CancellationPort;
  /** The device's storage, `localStorage`. */
  readonly deviceStorage: StoragePort;
  /** This window's storage, `sessionStorage`. */
  readonly windowStorage: StoragePort;
  /** `WebSocket`. */
  readonly socket: SocketPort<WebSocket>;
  /** `AudioContext` (or WebKit's prefixed one) and `Audio`. */
  readonly audio: AudioPort<typeof AudioContext, HTMLAudioElement>;
  /** `navigator.mediaDevices.getUserMedia`. */
  readonly microphone: MicrophonePort<MediaStreamConstraints, MediaStream>;
  /** `speechSynthesis` and `SpeechSynthesisUtterance`. */
  readonly speech: SpeechSynthesisPort<SpeechSynthesis, SpeechSynthesisUtterance>;
}

/**
 * The browser's ports: `fetch`, `setTimeout`/`clearTimeout`, `AbortController` where there is one,
 * `localStorage` and `sessionStorage`, whose very lookup throws where storage is refused, and the
 * socket, audio, microphone and device voice the voice call and reading aloud use.
 */
export function createWebPorts(): WebPorts {
  return {
    http: { fetch: (path: string, init: HttpRequestInit) => fetch(path, init as RequestInit) },
    clock: {
      setTimeout: (run: () => void, ms: number): TimerHandle => setTimeout(run, ms),
      clearTimeout: (handle: TimerHandle): void => clearTimeout(handle as ReturnType<typeof setTimeout>),
    },
    cancellation: { create: () => (typeof AbortController === "function" ? new AbortController() : null) },
    deviceStorage: {
      getItem: (key: string) => localStorage.getItem(key),
      setItem: (key: string, value: string) => localStorage.setItem(key, value),
      removeItem: (key: string) => localStorage.removeItem(key),
    },
    windowStorage: {
      getItem: (key: string) => sessionStorage.getItem(key),
      setItem: (key: string, value: string) => sessionStorage.setItem(key, value),
      removeItem: (key: string) => sessionStorage.removeItem(key),
    },
    socket: { open: (url: string) => new WebSocket(url) },
    audio: {
      contextClass: () => window.AudioContext || (window as PrefixedWindow).webkitAudioContext,
      element: (url: string) => new Audio(url),
    },
    microphone: { open: (constraints: MediaStreamConstraints) => navigator.mediaDevices.getUserMedia(constraints) },
    speech: {
      engine: () => {
        const engine = window.speechSynthesis;
        return engine && typeof engine.getVoices === "function" &&
          typeof engine.speak === "function" && typeof engine.cancel === "function" &&
          typeof window.SpeechSynthesisUtterance === "function" ? engine : null;
      },
      utterance: (text: string) => new window.SpeechSynthesisUtterance(text),
    },
  };
}
