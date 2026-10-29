// The API token this device signs in with (`#624 voice-page-modularity`, moved from the legacy
// script): its own handle, apart from every other setting, so nothing that lists or syncs settings
// can reach it.

import type { StoragePort } from "../../ports/index.ts";

/** Where the token is kept. */
const TOKEN_KEY = "vibe-talk.token";
/** Where this page kept it before the rename. */
const RENAMED_FROM_KEY = "gent-talk.token";

/** The device's token. */
export interface Credentials {
  /** The token, or "" for none. Throws where the device refuses storage. */
  token(): string;
  /**
   * True when there is no token under the current key but there is one under the key this page used
   * before the rename. Never throws: this runs on the sign-in path, where an exception would
   * replace the sign-in screen with nothing.
   */
  signedOutByRename(): boolean;
  /** Keep `value` as the token. Throws, or silently keeps nothing, where storage is refused. */
  keep(value: string): void;
  /** Forget the token. */
  forget(): void;
}

/** The token kept in `storage`. */
export function createCredentials(storage: StoragePort): Credentials {
  return {
    token: () => storage.getItem(TOKEN_KEY) || "",
    signedOutByRename(): boolean {
      // Every read is guarded: a browser with storage disabled throws on `getItem` rather than
      // returning null.
      try {
        return !storage.getItem(TOKEN_KEY) && !!storage.getItem(RENAMED_FROM_KEY);
      } catch (_error) {
        return false;
      }
    },
    keep(value: string): void {
      storage.setItem(TOKEN_KEY, value);
    },
    forget(): void {
      storage.removeItem(TOKEN_KEY);
    },
  };
}
