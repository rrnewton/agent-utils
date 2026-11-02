// The kinds of link under Links, and whether links are written as their addresses (`#212 link-filter`,
// `#266 link-filter-kinds`, `#620 link-url-chip`; `#624 voice-page-modularity`, moved from the legacy
// script into present/rows). The six kinds and their words, how a kind's count is printed, the kinds
// named in a sentence, and the one owner of the kinds the reader turned off and of URL, each kept on the
// device. What a kind is (`linkKind`) is web/src/foundation's; the buttons, keeping the reader's place,
// and the settings sync are the page's. Why there are six, what turning one off does, and why URL
// filters nothing, is told where the legacy script opens the section.

import type { StoragePort } from "../../ports/index.ts";

const LINK_KINDS_KEY = "vibe-talk.voice.hidden-link-kinds";

/** One kind: its name, its button's id, the word on it, and how a sentence names it where that differs. */
export interface LinkKind {
  readonly kind: string;
  readonly chip: string;
  readonly name: string;
  readonly said?: string;
}

/**
 * The kinds, in the order their buttons stand under Links, each with its button's id and the word
 * on it. The number of links of that kind in view is in `<chip>-count`, inside the button. `said` is
 * how a sentence names a kind whose word is not a plural of its own: "once Other links are hidden".
 * Other is last, and is what `linkKind` answers for every address no kind before it claims.
 */
const LINK_KINDS: readonly LinkKind[] = Object.freeze([
  { kind: "pr", chip: "link-kind-pr", name: "PRs" },
  { kind: "commit", chip: "link-kind-commit", name: "Commits" },
  { kind: "action", chip: "link-kind-action", name: "Actions" },
  { kind: "issue", chip: "link-kind-issue", name: "Issues" },
  { kind: "file", chip: "link-kind-file", name: "Files" },
  { kind: "other", chip: "link-kind-other", name: "Other", said: "Other links" },
]);

/**
 * The largest number a kind's button prints; past it, the number and a plus. It keeps each button
 * short: six of them are wider than a phone once their numbers are up, and the row scrolls sideways
 * there (web/voice.css), but no one button grows past what a phone shows of it at once.
 */
const LINK_KIND_COUNT_MAX = 99;

/** What a kind's button prints for `count` links: blank at none, so a kind with none is a plain word. */
function linkKindCountText(count: number): string {
  return count === 0 ? "" : count > LINK_KIND_COUNT_MAX ? `${LINK_KIND_COUNT_MAX}+` : String(count);
}

/**
 * The kinds turned off (`hidden`) that took something away — `counts` holds how many links of each kind
 * there are in what the sentence speaks for — named as their buttons are and joined as a sentence joins
 * them: "PRs", "PRs and Actions", "PRs, Commits and Other links". A kind that is off but had no
 * link there is left out, because turning it back on would bring nothing back. Every name is a
 * plural — Other is said as "Other links" — so "are" and "them" fit whichever it is.
 */
function hiddenKindWords(hidden: ReadonlySet<string>, counts: ReadonlyMap<string, number>): string {
  const names = LINK_KINDS.filter(({ kind }) => hidden.has(kind) && (counts.get(kind) as number) > 0)
    .map(({ name, said }) => said || name);
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** What the kinds and URL ask of the page. */
export interface LinkKindsDependencies {
  /** The device's storage (`deviceStorage`). */
  storage(): StoragePort;
  /** What this device kept for URL: on only for the word the page writes (`DEVICE_SETTINGS.linksRaw`). */
  rawStored(): boolean;
  /** Keep URL on or off; may throw where the browser refuses storage. */
  keepRaw(on: boolean): void;
}

/** The kinds' and URL's commands and questions. */
export interface LinkKinds {
  /**
   * The kinds the reader has turned off. Empty, every link shows. REPLACED, never changed in place,
   * so a row's drawn list can tell it is out of date by asking whether this is the set it was drawn
   * under (`showRowLinks`).
   */
  hidden(): ReadonlySet<string>;
  /** Show or hide one kind, and keep the choice. */
  setShown(kind: string, shown: boolean): void;
  /** Read the kinds back from the device: another device's choice arrived through the sync. */
  reload(): void;
  /** Whether the Links view writes each link as its address. */
  raw(): boolean;
  /** Write the links as their addresses, or as their names, and keep the choice. */
  setRaw(on: boolean): void;
  /** Read URL back from the device: another device's choice arrived through the sync. */
  reloadRaw(): void;
}

/** What this device kept, read now. */
export function createLinkKinds(deps: LinkKindsDependencies): LinkKinds {
  /**
   * What this device kept. Nothing kept is every kind on, and so is a record that does not parse;
   * a kind this page does not know is dropped, so one retired later cannot hide anything.
   */
  function storedHiddenLinkKinds(): Set<string> {
    let stored: unknown = null;
    try {
      stored = JSON.parse(deps.storage().getItem(LINK_KINDS_KEY) || "[]");
    } catch (_error) {
      stored = null;
    }
    const known = LINK_KINDS.map(({ kind }) => kind);
    return new Set(Array.isArray(stored) ? (stored as unknown[]).filter((kind): kind is string => known.includes(kind as string)) : []);
  }

  let hiddenLinkKinds = storedHiddenLinkKinds();

  /** Whether the Links view writes each link as its address. */
  let linksRaw = deps.rawStored();

  return {
    hidden: () => hiddenLinkKinds,
    setShown(kind, shown) {
      const hidden = new Set(hiddenLinkKinds);
      if (shown) hidden.delete(kind);
      else hidden.add(kind);
      hiddenLinkKinds = hidden;
      // In the order the buttons stand, so the same choice is always written the same way.
      const kept = LINK_KINDS.map((entry) => entry.kind).filter((one) => hidden.has(one));
      try {
        deps.storage().setItem(LINK_KINDS_KEY, JSON.stringify(kept));
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this page.
      }
    },
    reload() {
      hiddenLinkKinds = storedHiddenLinkKinds();
    },
    raw: () => linksRaw,
    setRaw(on) {
      linksRaw = on;
      try {
        deps.keepRaw(on);
      } catch (_error) {
        // A browser that refuses storage still honours the choice for this page.
      }
    },
    reloadRaw() {
      linksRaw = deps.rawStored();
    },
  };
}

// What the page and the tests use of it.
export { LINK_KINDS, LINK_KINDS_KEY, LINK_KIND_COUNT_MAX, hiddenKindWords, linkKindCountText };
