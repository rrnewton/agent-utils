// Writing the outbox package's copies to the device (`#193 compose-draft-loss`; `#624
// voice-page-modularity`): every owner here saves the same way.

import type { StoragePort } from "../../ports/index.ts";

/** `encoded` kept under `key`, read back to be sure: a refusing browser may drop a write silently. */
export function storedExactly(storage: StoragePort, key: string, encoded: string): boolean {
  try {
    storage.setItem(key, encoded);
    return storage.getItem(key) === encoded;
  } catch (_error) {
    return false;
  }
}
