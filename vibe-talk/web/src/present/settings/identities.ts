// What the channel box's list of who is who says (`#402 who-is-who-channel-box`, `#351 settings-polish`;
// `#624 voice-page-modularity`, moved from the legacy script into present/settings). Which bucket an
// account is in, and the census of who spoke where, are the page's; the list is web/src/view/settings's.

/** What each bucket an account can be put in is called in the list's picker. */
const BUCKET_LABELS: { readonly [bucket: string]: string | undefined } = Object.freeze({
  me: "Me (including my voice bot)",
  coder: "Coding agent",
  human: "Another person",
  bot: "Another bot",
});

/**
 * `#402 who-is-who-channel-box`. What the list says in place of its rows with All channels chosen:
 * a list of who spoke is one channel's. The owner may ask for the list hidden there instead, which is
 * the group hidden for All channels too, and this line gone.
 */
const IDENTITY_PICK_ONE = "Pick one channel to see who is who.";

/** One account the census has seen in a channel: its display name, and whether it is a bot. */
interface SeenAccount {
  readonly name: string;
  readonly bot: boolean;
}

/**
 * The accounts seen in a channel, in the list's order. People first, then bots, then by name: the
 * reader is looking for a name, and grouping the two kinds keeps the bridge and the coding agent beside
 * each other.
 */
function identityOrder(seen: { readonly [id: string]: SeenAccount }): string[] {
  return Object.keys(seen).sort((a, b) => {
    const left = seen[a] as SeenAccount;
    const right = seen[b] as SeenAccount;
    if (left.bot !== right.bot) {
      return left.bot ? 1 : -1;
    }
    return left.name < right.name ? -1 : left.name > right.name ? 1 : 0;
  });
}

/**
 * The list for a channel nobody has spoken in yet, naming the channel it is empty ABOUT: "nobody yet"
 * without a name reads as a broken panel rather than as an unread channel.
 */
const identityNobodyWords = (channel: string): string =>
  `Nobody yet in ${channel}. Read that channel, and everyone ` +
  "who has spoken in it appears here.";

/** Which channel the rows are about. */
const identityChannelWords = (channel: string): string => `In ${channel}:`;

/** What a row's picker is called to a screen reader: what the account is. */
const identityPickerLabel = (name: string, id: string): string => `what ${name || id} is`;

/** The line under the list once a choice in it is saved. */
const IDENTITY_SAVED = "Saved. This account is drawn this way from now on, in every channel it speaks in.";

export {
  BUCKET_LABELS,
  IDENTITY_PICK_ONE,
  IDENTITY_SAVED,
  identityChannelWords,
  identityNobodyWords,
  identityOrder,
  identityPickerLabel,
};
export type { SeenAccount };
