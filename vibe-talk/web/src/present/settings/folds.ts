// What Settings and Help fold, and what their one button says (`#346 settings-collapsible`, `#246
// help-sections`; `#624 voice-page-modularity`, moved from the legacy script into present/settings).
// Why they fold, and how, is told in web/src/view/settings, which draws them.

/**
 * Every fold on Settings, by slug: the five sections (`settings-<id>`) and every headed group in them.
 * Declared in the markup like every id here; the page suite holds this list to it in both directions.
 */
const SETTINGS_FOLDS: readonly string[] = Object.freeze([
  "settings-voice-agent",
  "sending",
  "instructions",
  "resuming",
  "relay",
  "reading-full",
  "canned-prompts",
  "microphone",
  "provider",
  "settings-messages",
  "mark-own",
  "mark-read",
  "auto-read",
  "empty-messages",
  "combining",
  "rich-headers",
  "message-links",
  "after-done",
  "summaries",
  "dismissed",
  "settings-channels",
  "hidden-channels",
  "settings-screen",
  "text-size",
  "reading-width",
  "collapsed-lines",
  "card-max-lines",
  "control-bar",
  "opening-links",
  "typing", "debug",
  "keyboard",
  "settings-storage",
  "storage",
  "message-cache",
  "token",
  "connection",
]);

/**
 * The folds that hold the pop-out over Recently dismissed (`#348 settings-dynamic-lists`): folding one
 * would leave it open out of sight, where Escape and Back would spend themselves closing it unseen.
 */
const FOLDS_HOLDING_DISMISSED: readonly string[] = Object.freeze(["dismissed", "settings-messages"]);

/**
 * The settings groups that have an explanation, and the slug that names it.
 *
 * ONE LIST, and three things are keyed off it: the `?` in the group is `#help-link-<slug>`, the
 * entry it opens is `#help-<slug>`, and this is what wires them together. Both elements are
 * declared in web/voice.html — the page suite refuses an id invented at runtime, and that rule is
 * why this is a list here rather than a scan of the document.
 *
 * The suite also asserts this list against the markup in both directions, so a group added with no
 * entry, or an entry nothing links to, fails rather than merely being unreachable.
 */
const HELP_TOPICS: readonly string[] = Object.freeze([
  "microphone",
  "control-bar",
  "keyboard",
  "canned-prompts",
  "identities",
  "channel-alias",
  "mark-own",
  "mark-read",
  "auto-read",
  "empty-messages",
  "combining",
  "rich-headers",
  "message-links",
  "summaries",
  "reading-width",
  "collapsed-lines",
  "card-max-lines",
  "resuming",
  "live-messages",
  "speech-prep",
  "storage",
  "message-cache",
  "voice-send",
  "voice-instructions",
  "connection",
]);

/**
 * Every entry of the Help screen, by slug: the ones a ? reaches (`HELP_TOPICS`), and the two read from
 * the screen itself.
 */
const helpEntrySlugs = (): string[] => ["controls", "channel", ...HELP_TOPICS];

/**
 * What the one button over a screen of folds says: Expand all while any fold on the page is folded,
 * Fold all once none is. It names what it will do, like every toggle on this page that is an act
 * rather than a mode.
 */
const foldAllWords = (anyFolded: boolean): string => (anyFolded ? "Expand all" : "Fold all");

export { FOLDS_HOLDING_DISMISSED, HELP_TOPICS, SETTINGS_FOLDS, foldAllWords, helpEntrySlugs };
