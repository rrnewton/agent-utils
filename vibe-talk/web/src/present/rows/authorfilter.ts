// What the author filter says (`#251 author-filter`, `#520 search-whose-toggle-hint`; `#624
// voice-page-modularity`, moved from the legacy script into present/rows): its three states in the
// order a press cycles them, the button's names and titles, the chip's words, and the sentence where
// rows would be when it leaves none. The filter in force, and the button and chip, are
// web/src/view/channel's (`createAuthorFilter`).

/** The author filter's states, in the order a press cycles them. */
const AUTHOR_FILTERS: readonly string[] = Object.freeze(["everyone", "mine", "theirs"]);

/** What the button says in each state: the state it is in, and where a press goes next. */
const AUTHOR_FILTER_LABELS: Readonly<Record<string, string>> = Object.freeze({
  everyone: "Everyone's messages. Press for only yours",
  mine: "Only your messages. Press for only everyone else's",
  theirs: "Only everyone else's messages. Press for everyone's",
});

const AUTHOR_FILTER_TITLES: Readonly<Record<string, string>> = Object.freeze({
  everyone: "Whose messages: Both (everyone's). Press to show only yours.",
  mine: "Whose messages: My messages (only yours). Press to show only everyone else's.",
  theirs: "Whose messages: Their messages (only everyone else's). Press to show everyone's.",
});

/** `#520 search-whose-toggle-hint`. The chip's words for each state: what a press landed on. */
const AUTHOR_FILTER_CHIP_LABELS: Readonly<Record<string, string>> = Object.freeze({
  everyone: "Both",
  mine: "My messages",
  theirs: "Their messages",
});

/** The next state round the cycle after `current`. */
function nextAuthorFilter(current: string): string {
  return AUTHOR_FILTERS[(AUTHOR_FILTERS.indexOf(current) + 1) % AUTHOR_FILTERS.length] as string;
}

/**
 * What the list says where its rows would be when the author filter has left none of the messages
 * loaded, or "" when it has not. `searching` is whether text is typed too, and `pinned` whether the
 * list is the Pinned filter's. With "Mark my own messages as read" on, the owner's messages start read,
 * so Hide read leaves them out and Mine under it is empty unless he put one back, and the sentence says
 * how to see them: `ownHiddenByRead` asks whether that is so (the channel's view, Hide read, and the
 * setting on).
 */
function authorEmptyWords(author: string, searching: boolean, pinned: boolean, ownHiddenByRead: () => boolean): string {
  if (author === "mine" && !pinned && ownHiddenByRead()) {
    return "Your own messages start read, so Hide read leaves them out — press h for Show read.";
  }
  const whose = author === "mine" ? "yours" : "from anyone else";
  if (pinned) {
    return searching
      ? `None of this channel's pinned messages ${author === "mine" ? "of yours" : "from anyone else"} match that search.`
      : `None of this channel's pinned messages are ${whose}.`;
  }
  return searching
    ? `None of the messages ${author === "mine" ? "of yours" : "from anyone else"} loaded so far match that search.`
    : `None of the messages loaded so far are ${whose}.`;
}

// What the page and the tests use of it.
export { AUTHOR_FILTERS, AUTHOR_FILTER_CHIP_LABELS, AUTHOR_FILTER_LABELS, AUTHOR_FILTER_TITLES, authorEmptyWords, nextAuthorFilter };
