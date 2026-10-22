# threads-view-main-messages: the main channel's messages in the Threads view

Working design, 2026-10-22. Not user documentation. Follows `#410 inline-reply-editor`
(2026-10-21-inline-reply.md), whose single moving editor it builds on.

## The ask

The owner, 2026-10-21 (via the coordinator): the Threads view (Cmd+3) shows only the messages that
have threads; the main channel's other messages are missing, and nothing there posts to the main
channel. Its Unread only and Unpack all sit below the search glass when they would fit beside it,
and there is room for one more: a toggle to show Main messages. With it on, the main channel's
messages that are not (yet) threads are in the list, and can be made threads from there: Thread or
Reply on one opens the inline reply under it instead of drilling into the thread view. And the main
channel's box is the implicit last card at the foot. One box on screen: Reply elsewhere moves it, and
closing a reply gives it back to the foot.

## What changes

1. **The heading on the glass's row.** In the Threads view across the window the heading (title,
   Unread only, Unpack all, Main messages) is drawn on the floating line the search glass sits on,
   clear of the glass, instead of a row of its own under it. While the search is open the bar has the
   line, and the heading goes back under it as now. Two columns are unchanged: there the right
   column's heading is its own row, over its own list.
2. **Main messages** (`#side-main`, key `a`, kept on this device like Unpack all): offered over the
   Threads view's cards. Pressed, the cards' list also holds the main channel's messages that head no
   thread with replies (the rows Main draws less its threads' roots), drawn as message rows by the
   code every list uses (`discordNode`, `dressChannelRow`), marked `data-main-row`, and placed among
   the cards in the list's order: a card where its thread last moved, a message where it was posted.
   Unread only keeps the ones still to do; the search finds them by their words; Hide read leaves out
   read ones as Main does.
3. **Thread and Reply on a Main row open the inline reply** under it (`#410`), bound for a new thread
   where the service starts one (the new-thread choice ticked; Slack starts one anyway), instead of
   drilling into the thread view.
4. **The main channel's box is the last card** while Main messages is on: `#channel-composer`, moved
   to the foot of the cards' list and posting to the main channel whatever view the left column was
   left on (its draft is the main channel's). It is the one box: a reply opened anywhere hides it, and
   putting the reply down gives it back.
5. Cards are told from message rows by `data-card` wherever the code meant "a card" (the lit card, the
   arrow from a row to its card, the last card for the room past the foot), not by "not unpacked".

## Tests

Page: the chip (markup, key, help, storage); Main rows among the cards in time order, threads' roots
left out, unread-only and search filtering them; Thread/Reply on a Main row opens the inline reply
under it with the new-thread choice ticked; the box at the foot posts to Main and is hidden while a
reply is open and back after; negative controls (off: no Main rows, no box at the foot). Chromium at
1280x800 and 393x852: the chips on the glass's row, the Main rows and the box at the foot, a reply
under a Main row moving the one box.
