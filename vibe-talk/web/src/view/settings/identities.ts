// Who is who, in the channel box (`#402 who-is-who-channel-box`, `#351 settings-polish`; `#624
// voice-page-modularity`, moved from the legacy script; plan step 16, view/settings).
//
// The Settings panel: one row per account seen IN THE CHANNEL THE CHANNEL BOX'S PICKER IS ON, saying
// what it is and letting that be changed.
//
// Rebuilt wholesale rather than patched, because the set of accounts grows as the channel is read and
// the guesses for accounts ALREADY listed can change when a new one arrives — the sole-other-bot rule
// is a statement about the whole census. A partial update would leave a stale "coding agent" beside the
// second bot that has just disqualified it.
//
// The channel is `#settings-channel`, the picker at the head of the box this list sits at the foot of,
// and NOT `#discord-channel`, which is what the reader is looking at: everything in that box is about
// the channel its picker names, and pointing it elsewhere navigates nowhere. `#349 channel-settings-all`
// had moved the list to Messages, following the channel being read, when the picker gained All channels;
// the owner, 2026-10-21, wanted it back "along with everything that varies depending on selecting the
// channel in the drop down". With All channels chosen it says so in one line (`IDENTITY_PICK_ONE`)
// instead of rows. Still one channel's census: a bot that only speaks somewhere else cannot spoil the
// guess here.
//
// `#351 settings-polish`. The line under the list (`#identity-state`) says what a choice just made in it
// did, and the choice was made in one channel's list: under another channel's it reads as a report about
// rows nobody has touched. So it goes when the list's channel changes (`identityListChannel`).
//
// Which bucket an account is in, the census, and the reader's choices are web/src/core/channels's
// (`createIdentities`); the list reads them through `IdentityReader` and chooses through the page.

import {
  BUCKET_LABELS,
  IDENTITY_PICK_ONE,
  IDENTITY_SAVED,
  identityChannelWords,
  identityNobodyWords,
  identityOrder,
  identityPickerLabel,
} from "../../present/settings/index.ts";
import type { SeenAccount } from "../../present/settings/index.ts";
import type { IdentityReader } from "../../core/channels/index.ts";

/** What the list asks of the page. */
export interface IdentityListDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** The picker's value for All channels (`ALL_CHANNELS`). */
  allChannels(): string;
  /** Whether the channel with this id is on the list (`knownChannel`). */
  known(id: string): boolean;
  /** What the page calls the channel with this id (`channelName`). */
  nameOf(id: string): string;
  /** Who is who: the census, the buckets, and which one each account is in (`whoIsWho`). */
  readonly who: IdentityReader;
  /** The reader chose a bucket for this account: kept on the device (`whoIsWho.choose`). */
  choose(id: string, bucket: string): void;
  /** Redraw the channel's rows, which a choice changes (`renderChannelRows`). */
  renderChannelRows(): void;
}

/** The list's one command. */
export interface IdentityList {
  /** Rebuild the list for the channel the box's picker is on. */
  render(): void;
}

/** Drawn for no channel yet. Made once, where the page keeps the census. */
export function mountIdentityList(deps: IdentityListDependencies): IdentityList {
  /** The channel the list of who is who was last drawn for (`render`), or null before it was. */
  let identityListChannel: string | null = null;

  function render(): void {
    const host = deps.lookup("identity-list");
    const channelId = (deps.lookup("settings-channel") as HTMLSelectElement).value;
    if (String(channelId) !== identityListChannel) {
      deps.lookup("identity-state").textContent = "";
      identityListChannel = String(channelId);
    }
    // No channel to be about (a service with none on the list, whose box already says how to add one):
    // no heading over nothing.
    const none = channelId !== deps.allChannels() && !deps.known(channelId);
    deps.lookup("identity-group").hidden = none;
    if (none) {
      host.replaceChildren();
      return;
    }
    if (channelId === deps.allChannels()) {
      const pick = document.createElement("p");
      pick.className = "hint";
      pick.textContent = IDENTITY_PICK_ONE;
      host.replaceChildren(pick);
      return;
    }
    const seen: { readonly [id: string]: SeenAccount } = deps.who.seenIn(channelId);
    const ids = identityOrder(seen);
    if (ids.length === 0) {
      const empty = document.createElement("p");
      empty.className = "hint";
      // Names the channel through the one rule that names channels — so this says whatever the
      // picker above it says. The list changes when that picker does.
      empty.textContent = identityNobodyWords(deps.nameOf(channelId));
      host.replaceChildren(empty);
      return;
    }
    // Which channel the rows are about, through the one rule that names channels.
    const about = document.createElement("p");
    about.className = "hint identity-channel";
    about.textContent = identityChannelWords(deps.nameOf(channelId));
    const rows = ids.map((id) => {
      const who = seen[id] as SeenAccount;
      // A <label> WRAPPING its control, rather than a <div> plus `for="identity-<id>"`.
      //
      // The id would have to be minted from the author's snowflake at runtime, and the page's test
      // fixture derives the element set from web/voice.html by regex and refuses anything else —
      // deliberately, so that an element invented at runtime fails loudly instead of silently at the
      // roadside. That rule is worth more than the attribute: a label containing its control is
      // associated with it implicitly, which is the same accessibility outcome with no id at all.
      const wrap = document.createElement("label");
      wrap.className = "identity-row";

      const label = document.createElement("span");
      label.className = "identity-name";
      // textContent, not innerHTML: a display name is channel data like any other, and this panel is
      // the one place the page renders one outside the message list.
      label.textContent = who.name || "(no name)";
      if (who.bot) {
        const tag = document.createElement("span");
        tag.className = "identity-tag";
        tag.textContent = "bot";
        label.append(tag);
      }
      if (deps.who.isSelfId(id)) {
        const tag = document.createElement("span");
        tag.className = "identity-tag";
        // Says WHY this one is you, so "me" on a bot account does not read as a mistake.
        tag.textContent = "this bridge";
        label.append(tag);
      }

      const select = document.createElement("select");
      // Named for a screen reader without an id, since the wrapping label is doing the association.
      select.setAttribute("aria-label", identityPickerLabel(who.name, id));
      // The snowflake, so a handler and the suite can both say WHICH row this is without an id.
      select.setAttribute("data-author-id", id);
      for (const bucket of deps.who.buckets()) {
        const option = document.createElement("option");
        option.value = bucket;
        option.textContent = BUCKET_LABELS[bucket] as string;
        select.append(option);
      }
      select.value = deps.who.bucketFor(id, who.bot, channelId);
      // Whether the value on screen is a CHOICE or a GUESS, which is the difference between "this is
      // wrong" and "nobody has said". Without it a wrong guess is indistinguishable from a decision.
      // Always set, both ways, rather than present-or-absent: "false" and "missing" are the same
      // thing to a stylesheet but not to anything reading the attribute back, and a reader asking
      // "was this a guess?" should get an answer rather than an absence.
      select.setAttribute("data-guessed", deps.who.chosen(id) ? "false" : "true");
      select.addEventListener("change", () => {
        deps.choose(id, select.value);
        select.setAttribute("data-guessed", "false");
        deps.lookup("identity-state").textContent = IDENTITY_SAVED;
        deps.renderChannelRows();
        // The row just changed can change the GUESS on its neighbours: naming one of two bots as the
        // coding agent leaves the other as the only unnamed candidate. Only this channel's list is
        // on screen, so only this channel's list is rebuilt — but rebuilt it must be, or the panel
        // shows a stale guess beside the choice that invalidated it.
        render();
      });

      const id_ = document.createElement("span");
      id_.className = "identity-id";
      id_.textContent = id;

      wrap.append(label, select, id_);
      return wrap;
    });
    host.replaceChildren(about, ...rows);
  }

  return { render };
}
