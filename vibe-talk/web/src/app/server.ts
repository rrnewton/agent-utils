// The page's way to the browser and the server (`#624 voice-page-modularity`, moved from
// the legacy script): the browser's ports, the storages and settings on them, and the
// requests to vibe-talk, each made on first use exactly as the script made them. What a
// request needs that only the page knows — the token, the refusal's clearing, the read
// count, who did not answer — comes in as `deps`, asked at the moment of use.

import { createWebPorts } from "../platform/web/index.ts";
import type { WebPorts } from "../platform/web/index.ts";
import { createApi } from "../core/api/index.ts";
import type { ApiRequestOptions, ServerApi } from "../core/api/index.ts";
import { createCredentials, createDeviceSettings } from "../core/settings/index.ts";
import type { Credentials, DeviceSettings } from "../core/settings/index.ts";
import { sentenceStart } from "../foundation/index.ts";
import type { CancelSignal, ClockPort, StoragePort, TimerHandle } from "../ports/index.ts";
import type { Contract } from "../contract/index.ts";

/** What the page's server access asks of the page, at the moment of use. */
export interface ServerAccessDependencies {
  readRefused(): void;
  noteAsked(): void;
  chatServiceOf(channel: unknown): string;
  selectedChannelId(): unknown;
}

/** The browser's ports, the page's storages and settings, and its requests. */
export interface ServerAccess {
  browserPorts(): WebPorts;
  deviceStorage(): StoragePort;
  deviceStorageOnUse(): StoragePort;
  clockOnUse(): ClockPort;
  windowStorage(): StoragePort;
  deviceSettings(): DeviceSettings;
  credentialStore(): Credentials;
  token(): string;
  signedOutByRename(): boolean;
  theServer(): ServerApi;
  api(path: string, options?: ApiRequestOptions): Promise<unknown>;
  apiDecoded<K extends keyof Contract>(type: K, path: string, options?: ApiRequestOptions): Promise<Contract[K]>;
  within<T>(ms: number, request: (signal: CancelSignal | undefined) => Promise<T>, abandoned?: ((answer: Promise<T>) => void) | null): Promise<T>;
}

/** The server access over `deps`; everything in it is made on first use. */
export function createServerAccess(deps: ServerAccessDependencies): ServerAccess {
  /**
   * The browser's capabilities as the page's modules take them (web/src/platform/web, `#624
   * voice-page-modularity`): transport, timers, cancellation and storage. Each looks its browser
   * global up at every call, as the page's own calls did, so a browser refusing storage throws at the
   * same moment it always did. Made on first use; a `var` with no initialiser, like `serverApi`.
   *
   */
  let webPorts: WebPorts | undefined;

  /** {@link webPorts}, made if it is not yet. */
  function browserPorts(): WebPorts {
    if (!webPorts) webPorts = createWebPorts();
    return webPorts;
  }

  /** The device's storage, `localStorage`, which every window of this origin shares. */
  function deviceStorage(): StoragePort {
    return browserPorts().deviceStorage;
  }

  /**
   * {@link deviceStorage} for an owner made before the ports are: each call looks the port up, so making
   * the owner makes no port early.
   *
   */
  function deviceStorageOnUse(): StoragePort {
    return {
      getItem: (key: string) => deviceStorage().getItem(key),
      setItem: (key: string, value: string) => deviceStorage().setItem(key, value),
      removeItem: (key: string) => deviceStorage().removeItem(key),
    };
  }

  /**
   * The page's clock for an owner made before the ports are, as {@link deviceStorageOnUse} is its storage.
   *
   */
  function clockOnUse(): ClockPort {
    return {
      setTimeout: (run: () => void, ms: number) => browserPorts().clock.setTimeout(run, ms),
      clearTimeout: (handle: TimerHandle) => browserPorts().clock.clearTimeout(handle),
    };
  }

  /** This window's own storage, `sessionStorage`. */
  function windowStorage(): StoragePort {
    return browserPorts().windowStorage;
  }

  /**
   * The settings this device keeps (web/src/core/settings `DEVICE_SETTINGS`), in its storage: each
   * setting's key, encoding and default said once there. Made on first use, like `webPorts`.
   *
   */
  let pageDeviceSettings: DeviceSettings | undefined;

  /** {@link pageDeviceSettings}, made if it is not yet. */
  function deviceSettings(): DeviceSettings {
    if (!pageDeviceSettings) pageDeviceSettings = createDeviceSettings(deviceStorage());
    return pageDeviceSettings;
  }

  /**
   * The device's API token, on its own handle (web/src/core/settings, `#624 voice-page-modularity`),
   * kept in the device's storage. Made on first use, like `serverApi`.
   *
   */
  let pageCredentials: Credentials | undefined;

  /** {@link pageCredentials}, made if it is not yet. */
  function credentialStore(): Credentials {
    if (!pageCredentials) pageCredentials = createCredentials(deviceStorage());
    return pageCredentials;
  }

  /** The token, or "" for none. Throws where the browser refuses storage. */
  const token = (): string => credentialStore().token();

  /**
   * True when this browser has no token under the current key but does have one under the key this
   * page used before the rename. Never throws (`Credentials.signedOutByRename`).
   */
  function signedOutByRename(): boolean {
    return credentialStore().signedOutByRename();
  }

  /**
   * The page's requests to vibe-talk (web/src/core/api, `#624 voice-page-modularity`), on the
   * browser's transport, timers and cancellation, made the first time the page asks anything.
   *
   */
  let serverApi: ServerApi | undefined;

  /** {@link serverApi}, made if it is not yet. */
  function theServer(): ServerApi {
    if (!serverApi) {
      serverApi = createApi(browserPorts(), {
        token,
        // A token the server no longer accepts, or a read it refuses, ends this device's copy of
        // what that token read.
        readRefused: () => deps.readRefused(),
        // `remote-ops-audit` W7. Every timeline read the page sends passes here, and is counted as it
        // is sent, so a read given up on can tell whether any was sent after it (`abandonedReadStands`).
        asking: (type: keyof Contract) => {
          if (type === "TimelineResponse") deps.noteAsked();
        },
        answerer: () => sentenceStart(deps.chatServiceOf(deps.selectedChannelId())),
      });
    }
    return serverApi;
  }

  /**
   * One request to vibe-talk, with the token, and one error taxonomy out of it: every field of the
   * server's error answer passed through (web/src/core/api `request`).
   *
   */
  function api(path: string, options?: ApiRequestOptions): Promise<unknown> {
    return theServer().request(path, options);
  }

  /**
   * {@link api}, with the answer checked against the wire contract before anything reads it
   * (web/src/core/api `decoded`).
   *
   */
  function apiDecoded<K extends keyof Contract>(type: K, path: string, options?: ApiRequestOptions): Promise<Contract[K]> {
    return theServer().decoded(type, path, options);
  }

  /**
   * A request to vibe-talk that stops waiting after `ms`, failing as a timeout the reader can be
   * told about rather than as a "refreshing…" nobody ends (web/src/core/api `within`, `#195
   * send-resilience`). The poll does not read that view again until the read answers, the server's
   * own bound has passed, or a live event says the service is answering (`readHeld`,
   * `remote-ops-audit` W7).
   *
   * `abandoned`, when given, is handed the request still on the wire when the wait is given up, and
   * the request is then not aborted: its answer, if it comes, is the caller's to use or drop
   * (`abandonRead`). Without it the request is aborted, as it always was.
   *
   */
  function within<T>(ms: number, request: (signal: CancelSignal | undefined) => Promise<T>, abandoned?: ((answer: Promise<T>) => void) | null): Promise<T> {
    return theServer().within(ms, request, abandoned);
  }

  return {
    browserPorts, deviceStorage, deviceStorageOnUse, clockOnUse, windowStorage,
    deviceSettings, credentialStore, token, signedOutByRename, theServer, api, apiDecoded, within,
  };
}
