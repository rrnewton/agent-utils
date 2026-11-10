// A message's details sheet: who sent it, when, its edit and its id, and the bookmark
// (`#624 voice-page-modularity`, moved from the legacy script). Words and order are as
// they were in the script. The times and the edited mark are present's, the child
// lookup view/dom's; the bookmark acts stay the page's, handed in.

import { fullLocalAt, fullLocalTime } from "../../present/time/index.ts";
import { rowEdited } from "../../present/rows/index.ts";
import { childByClass } from "../dom/index.ts";
import type { Message } from "../../contract/index.ts";

/** What opening a message's details asks of the page. */
export interface MessageDetailsDependencies {
  bookmarkAbove(row: HTMLElement): boolean;
  toggleBookmarkAt(row: HTMLElement): void;
  inThisColumn(act: () => void): () => void;
}

/** Opening and closing a message's details sheet. */
export interface MessageDetails {
  toggleMessageDetails(row: HTMLElement, messages: Message[]): void;
}

/** The message details sheet over `deps`. */
export function createMessageDetails(deps: MessageDetailsDependencies): MessageDetails {
  function toggleMessageDetails(li: HTMLElement, messages: Message[]): void {
    const message = messages[0] as Message;
    const open = childByClass(li, "msg-details");
    if (open) {
      li.removeChild(open);
      return;
    }
    const details = document.createElement("div");
    details.className = "msg-details";
    const who = document.createElement("div");
    who.textContent = message.author_is_bot
      ? `${message.author} (bot)`
      : String(message.author || "");
    const when = document.createElement("div");
    when.textContent = fullLocalTime(message);
    // `#320 slack-edited-mark`. When it was last edited, which a phone cannot hover the mark to learn.
    const edited = rowEdited(messages);
    const editedLine = document.createElement("div");
    editedLine.className = "msg-edited";
    editedLine.textContent = edited === null ? "" : edited.at === null ? "Edited" : `Edited ${fullLocalAt(edited.at)}`;
    const id = document.createElement("div");
    id.className = "msg-id";
    // EVERY id, on a row that is showing more than one message. This sheet is the answer to "does
    // that message really exist", and a combined row that named only its first constituent would
    // make the second unreachable by exactly the affordance built to reach it — while also hiding
    // the fact that the row is two messages at all.
    id.textContent = `id ${messages.map((m) => String(m.id)).join(" + ")}`;
    // THE BOOKMARK lives here as well as in the ⋯ menu and on Shift+B, rather than on a gesture of its
    // own. Press-and-hold already opens this sheet, and a second long-press meaning something
    // different from the first would be a gesture nobody could discover and everybody would trigger by
    // accident. `#240 reading-bookmark`.
    const mark = document.createElement("button");
    mark.className = "chip";
    mark.setAttribute("type", "button");
    mark.textContent = deps.bookmarkAbove(li) ? "Remove bookmark" : "Bookmark above";
    mark.addEventListener("click", deps.inThisColumn(() => {
      deps.toggleBookmarkAt(li);
      const open = childByClass(li, "msg-details");
      if (open) li.removeChild(open);
    }));
    details.append(who, when, ...(edited === null ? [] : [editedLine]), id, mark);
    li.append(details);
  }

  return { toggleMessageDetails };
}
