// Showing rich headers in a row's header line, and what was stored (`#624
// voice-page-modularity`, moved from the legacy script). Words and order are as they
// were in the script. The settings record and its descriptor stay core/settings',
// made in the script and handed in — the descriptor is passed whole, so the stored
// key and values cannot drift from what the page has always kept.

/** The device settings record, as this owner uses it. */
export interface RichHeaderSettings {
  get(descriptor: unknown): boolean;
  keep(descriptor: unknown, value: boolean): void;
}

/** What showing rich headers asks of the page. */
export interface RichHeadersDependencies {
  renderRichHeaders(shown: boolean): void;
  deviceSettings(): RichHeaderSettings;
  richHeadersSetting: unknown;
}

/** Showing rich headers, and what was stored. */
export interface RichHeaders {
  applyRichHeaders(on: boolean): void;
  storedRichHeaders(): boolean;
}

/** The rich headers switch over `deps`. */
export function createRichHeaders(deps: RichHeadersDependencies): RichHeaders {
  /**
   * Show headings in the header, or draw every first line as written, and remember which. The root's
   * attribute is the whole of the switch: nothing else here asks whether it is on.
   */
  function applyRichHeaders(on: boolean): void {
    const shown = Boolean(on);
    deps.renderRichHeaders(shown);
    try {
      deps.deviceSettings().keep(deps.richHeadersSetting, shown);
    } catch (_error) {
      // A browser that refuses storage still honours the choice for this session.
    }
  }

  /** What was stored. ABSENT MEANS ON: the default is the behaviour, not merely the initial value. */
  function storedRichHeaders(): boolean {
    // A browser refusing storage reads as on (`deps.richHeadersSetting.refused`).
    return deps.deviceSettings().get(deps.richHeadersSetting);
  }

  return { applyRichHeaders, storedRichHeaders };
}
