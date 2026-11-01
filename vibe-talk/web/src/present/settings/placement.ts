// Where the control bar sits, and what Settings says when it moves (`#58 control-bar`; `#624
// voice-page-modularity`, moved from the legacy script into present/settings). The bar and the device's
// copy of the choice are web/src/view/settings's.

/** The places the bar can be: in the dock above the big buttons, or under the title. */
const PLACEMENTS: readonly string[] = Object.freeze(["bottom", "top"]);

/**
 * Bottom, because that is where the thumb already is — and because with the bar down there the header
 * holds nothing on the main screen and can be collapsed entirely, which is the vertical space the issue
 * is actually about.
 */
const DEFAULT_PLACEMENT = "bottom";

/** A place the bar can be: `value`, validated, or the default. Anything else is somebody else's data. */
const placementOf = (value: unknown): string => (PLACEMENTS.includes(value as string) ? value as string : DEFAULT_PLACEMENT);

/** What Settings says of the bar just moved to `placement`: kept, or refused by the browser and moved for now. */
const placementWords = (saved: boolean, placement: string): string => saved
  ? placement === "top"
    ? "Saved. The bar is under the title again."
    : "Saved. The bar sits directly above the big buttons, and the header takes no room."
  : "This browser refused to store the setting, so it will be back at the bottom when you " +
    "reload (private browsing does this). The bar has moved for now.";

export { DEFAULT_PLACEMENT, PLACEMENTS, placementOf, placementWords };
