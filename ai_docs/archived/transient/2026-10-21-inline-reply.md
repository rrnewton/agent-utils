# inline-reply-editor: Reply opens in the stream, under the message and its replies

Working design, 2026-10-21. Not user documentation.

## The ask

The owner, 2026-10-21: pressing Reply on a message inside a thread in two columns opens a reply box
at the foot of the column, while the thread's own "Reply in this thread…" box is still drawn at the
foot of its messages: two boxes. In one column Reply goes to a different screen that draws the
messages differently, and in a thread it shows what came before the message and not its replies. He
asks for one kind of box, in the stream: Reply gathers the message's replies under it with the bridge
used elsewhere, and the page's inline message editor, with its Grow button, opens under them, in
the middle of the list if the message is old. Later messages stay below it.

## What changes

1. **No reply screen at all.** Reply, Quote reply, a row's Draft and a failed reply's Edit, pressed
   on a row of Main, All, a thread (drilled in, or in the right column), the Threads view's unpacked
   rows or the Pinned filter, open the reply in that list, in one column or two, at any width. The
   page already had this for two columns (`#277 reply-in-column`'s "docked" reply), with its
   lifecycle settled: put away when the list changes under it, Back closes it, the keys go back to
   the list. That machinery is reused and only *where* the box goes changes: under the message
   instead of at the column's foot. The reply screen (`#51 reply-view`, `#200 reply-context`, `#247
   reply-continuity`) is deleted, with the tests that pinned it (coordinator, 2026-10-21: "delete").
   Two columns that go while a reply is open put it away as a draft and open it again in the stream
   as soon as its message is drawn: at once in the one column left where it is drawn there, or later
   when the reader comes back to it (`pendingReply`, `reopenPendingReply`).

2. **The editor is the reply's own box, laid out as the page's inline composer.** There is one reply
   box (`#reply-compose`). It carries everything a reply needs that the channel's composer cannot:
   the message it answers, the per-message draft (`#193`), the quote (`#267`), the new-thread choice
   and where it posts (`#185`), the removed-message refusal (`#315`), the Done a reply makes
   (`#300`). It is already one of the composer fields: Enter and Ctrl+Enter (`#225`), Ctrl+S and c
   (`#248`, `#403`). It gains the composer's Grow checkbox, which joins `#340`'s one choice
   (`COMPOSER_GROW_BOXES`), with the same fit and keep-in-view code, and the composer's layout: the
   box across, Grow at the start of the row under it, Send at its end, plus Cancel and the line
   naming who is answered. Moving the channel composer itself into the stream would need a second
   send path for replies; reusing the reply box keeps one.

3. **Where the box goes.** The box stays a child of the column's scrolling pane (never a child of
   a message list: 85 walkers read a list's children as its rows, and some replace them wholesale,
   which would take the box out of the document). It is drawn under its *anchor row* by CSS anchor
   positioning (`anchor-name` on the row, `position-anchor` on the box), and the anchor row is given
   room under itself equal to the box's height (`--inline-reply-room`, kept by a ResizeObserver on
   the box). So the rows below move down by exactly the box, the box follows its row through every
   fold, read run, live arrival and older page above it with no script, and every list keeps only
   rows. Where the browser has no anchor positioning, or the anchor row is not drawn (a search hides
   it), the box is drawn in the list's flow at its foot, where the composer was.

4. **Gathering, by the code each view already has.**
   - All: `#215 reply-coalesce`'s gathering (`gatherNow`), as N replies does: the replies stacked
     under the message, bridged, the message kept where it was on the screen.
   - A thread, drilled in or on the right, and Main: the same gathering, allowed there for the reply
     (`gathered.byReply`): replies by pointer to the message come up under it. The thread's own root
     already has every reply under it, so its anchor is the thread's last row.
   - A thread root in Main: its replies are not Main's rows, so the gathering brings them in as rows
     of Main's list while it stands (`gathered.thread`, `data-gathered-thread`), chosen by `#407`'s
     own selection (`unpackMessages`: what a thread draws, under the read mode) and drawn, dressed and
     bridged by the list's own code, so every row rule, key and bridge applies. They are marked
     `data-gathered-thread`, and what means "Main's messages" leaves them out: the saved snapshot
     (`listedMessages`), the Hide read sync and the bookmark line. Done through here covers the rows
     above it on the screen, a gathering's included, as `#215` decided for All. (A nested list in the
     reply's place was tried first: it would need a twin of every `#discord-log` row rule.)
   - The Threads view with Unpack all: the block is already the thread; the anchor is the row, or the
     block's last row for its root.
   - The anchor is the last row of the stack under the message, or the message's own row.

5. **One box at a time.** While a reply is open, the columns' composers are hidden, both of them in
   two columns; Send and Cancel bring them back, and Reply elsewhere moves the one box there. Drafts stay per target: the reply's per message (`drafts`), the
   composer's per channel and thread (`channelDrafts`), untouched by either.

6. **Closing and sending.** Esc, Cancel and Back put the reply away, its words kept as the message's
   draft, and put back a gathering the reply made (the message held where it is). Send closes the box
   and leaves the gathering, so the reply joins the stack when it lands; a Main root's unpacked
   replies stay under it until the reader puts them away (their X), another Reply, or a change of
   view. The reader's place is held through both: the column is not restored to an old scroll mark
   (which would jump), the message answered stays where it was, and a reader at the foot is kept there
   (`followFoot`).

7. **Keys.** r opens it, Esc closes it, Ctrl/Cmd+S and c go to the open box, as before. Help says the
   reply opens under the message.

## What the reply screen gave, and where it is now

| Reply screen (`#200`, `#247`) | Inline |
|---|---|
| The message drawn as its row, same size and width (`#247`) | It IS the row, unmoved. |
| The message never folded: answering it is the reason (`#51`) | No longer needed. The reply screen drew the message with nothing on it that acts, so it could not be opened there; in the list the row keeps its own More and Enter, where the reader is. (Opening it on Reply was tried: folding it again on close moved a reader of a tall message far from where they were.) |
| Its top kept where the row's was (`#247`) | Nothing moves; gathering holds the row (`holdRowAt`). |
| The box directly under the message (`#247`) | Under the message, or under its gathered replies. |
| Low on the screen, moved up only as far as shows the box; a tall message keeps its top (`#247`) | `dockReply` scrolls by as little as shows the whole box. A message taller than the room goes up, its top a scroll up: the box, which the reader types into, is never left under the screen's foot. |
| Placed again on resize and when the note under the box changed (`#247`) | Anchor positioning follows the row; the room under it follows the box (`fitInlineRoom`). |
| Cancel/Back/Esc back to the pixel (`#247`) | `leaveDockedReply` holds the message, or the reader at the foot. |
| The tint that says "this is Reply" (`#247`) | The box is drawn on `--reply-tint`. |
| Earlier messages of the thread above the message (`#200`) | The whole list is above it, and its replies are gathered under it, which the screen never showed. |
| Load earlier messages (`#200`) | The list's own Older messages and scrollback paging. |
| The title naming the thread the reply goes into (`#200`) | The list it is in names it (the thread's card, Main, All); the note under the box says where it posts, a new thread or a thread not known yet included. |
| The call's controls put away while replying (`#51`) | No longer needed: the reply is on the channel's list, with the channel's controls. |

## Tests

- Page (fake DOM): the box is in the pressed column's pane and the columns' composers hidden while it
  is open, in one column and two (fails before: the reply screen in one column; the composer still
  drawn beside the docked box); the anchor row carries the anchor and room; All gathers the
  message's replies under it and Esc scatters them; Main's root unpacks its thread above the box;
  negative controls: the composer back after Cancel and after Send, drafts kept per message and per
  channel.
- Chromium, 1280x800 two columns and 393x852: reply to an old message mid-stream, the box directly
  under the gathered replies, later rows below, one box on screen, the message answered not moved;
  Esc restores the composer; Send places the reply without a jump. Screenshots kept for review.
