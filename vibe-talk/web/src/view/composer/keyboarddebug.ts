// The keyboard layout debug choice, kept on this device (`#624 voice-page-modularity`,
// moved from the legacy script). Words and order are as they were in the script. The
// settings record and its descriptor stay core/settings', handed in whole so the
// stored key and values cannot drift; drawing the numbers is the screen keyboard's.

/** The device settings record, as this owner uses it. */
export interface KeyboardDebugSettings {
  get(descriptor: unknown): boolean;
  keep(descriptor: unknown, value: boolean): void;
}

/** What the keyboard debug choice asks of the page. */
export interface KeyboardDebugDependencies {
  deviceSettings(): KeyboardDebugSettings;
  keyboardDebugSetting: unknown;
  showKeyboardDebug(on: boolean): void;
}

/** The keyboard debug choice, stored and applied. */
export interface KeyboardDebug {
  storedKeyboardDebug(): boolean;
  applyKeyboardDebug(on: boolean): void;
}

/** The keyboard debug choice over `deps`. */
export function createKeyboardDebug(deps: KeyboardDebugDependencies): KeyboardDebug {
  /** What was stored. ABSENT MEANS OFF. */
  function storedKeyboardDebug(): boolean {
    // A browser refusing storage reads as off (`deps.keyboardDebugSetting.refused`).
    return deps.deviceSettings().get(deps.keyboardDebugSetting);
  }

  /** The reader's choice, kept on this device. */
  function applyKeyboardDebug(on: boolean): void {
    try {
      deps.deviceSettings().keep(deps.keyboardDebugSetting, on);
    } catch (_error) {
      // A browser that refuses storage still draws it for this session.
    }
    deps.showKeyboardDebug(on);
  }

  return { storedKeyboardDebug, applyKeyboardDebug };
}
