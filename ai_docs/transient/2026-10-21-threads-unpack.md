# Threads unpack-all and root-under-summary — design note

Date: 2026-10-21. Owner request (verbatim in the task brief): in the Threads
view (Cmd+3), a toggle that unpacks every thread's messages under its card at
once, in the connected/bridged style of the All view's reply bridge,
respecting Show / Collapse / Hide read; and, wherever a summarized card heads
a thread, the raw root message shown as the first child under that card.

## What exists (read before designing)

- **Reply bridge** (`#215 reply-coalesce`): in All, **N replies** gathers one
  message's replies under it. One parent at a time (`gathered` in
  `vibe-talk/web/voice.js`). Gathered rows carry `data-coalesce`
  (parent/first/middle/last/only) and `data-stack-under` (the parent's shape:
  coder/me/third/reply/me-reply). Each gathered row gets a `.reply-bridge`
  span (spine + stub into the box) and the first gets `.reply-ungather` (the
  X). `renderReplyLinks()` assigns places after every redraw. CSS draws the
  bridge in `#discord-log` and `#side-log` only.
- **Threads view** (Cmd+3, `#281 view-keys`): `threadListShown` puts the side
  column across the window. Cards are `#side-cards`, built by
  `renderSideCards()` / `sideCard()` from `threadChoices()` (newest activity
  first, reversed so newest is at the foot). Heading `#side-heading` holds
  **Unread only** (`#259`), a chip kept per device in
  `vibe-talk.voice.side-unread-only`. The same cards serve the right column in
  two columns; unpack-all must therefore gate on `threadListShown` so two
  columns are unchanged.
- **Summarized card**: `sideCardText(choice)` is `"<display_name>:
  <summary>"` when the summariser named and summed the thread, else the root's
  own text. `sideCardRootText(choice)` is non-empty exactly when the card says
  something other than the root — the summarized/raw distinction both features
  key on. Cards at the top of an open thread (`#298`, `paintThreadCard`) use
  the same words.
- **Thread drill-in**: `channelView === "thread"` projects
  `inView(message, "thread", id)` — root included. Under **Hide read** the
  root vanishes when read (`readFilterKeeps` keeps only `stillToDo` outside
  Main); under **Collapse read** the root is protected by
  `isOpenThreadRoot`. So "always see the first message" fails today under Hide
  read, and the root is not drawn as a child of the card anywhere.
- **Read runs** (`#221`): `applyReadRuns()` collapses runs of ≥2 read rows in
  `#discord-log` into a `.read-run` line ("… N read messages …") on the head
  row; gathered rows (`data-coalesce`) are exempt so a line never breaks the
  bridge. Rows are built by `discordNode(glom(messages))`; `glom` drops
  hidden-empty messages (`#386`) and combines neighbours. `stillToDo` is the
  one read test. `#385`'s gutter: thread rows in the right column keep a
  left gutter so the column link never crosses boxes — unpacked rows must not
  eat that gutter.

## (1) threads-unpack-all

- **Toggle**: chip **Unpack all** in `#side-heading` beside **Unread only**,
  `aria-pressed`, shown only while the cards are up. Key **`u`** in `KEYMAP`
  (`KEYS_VIEW`, `controls: ["side-unpack"]`) — free today, mnemonic, single
  key like `h` (read modes) and `t` (thread strip). Listed automatically in
  the **?** help via `renderKeyHints`. State kept per device in
  `vibe-talk.voice.threads-unpacked` ("on"/"off"), mirroring
  `storedSideUnreadOnly` / `THREAD_CARD_FOLD_KEY`. Default off.
- **Rendering**: while on and `threadListShown`, each card row is followed (as
  later siblings in `#side-cards`, not nested, so card layout/key walks that
  read `data-card` rows are untouched) by its thread's message rows built with
  `discordNode(glom(...))` in time order, oldest first, each `.unpack-row`
  carrying `data-coalesce` + `data-stack-under="third"`-style indent and a
  `.reply-bridge` piece whose spine is tinted by the card's `--thread-hue`
  (the bridge's `color` reads a per-thread variable, falling back to
  `--accent`). No ungather X: the toggle is the way back. Threads keep
  `threadChoices()` last-updated order; cards do not move.
- **Read modes inside a block**: computed per thread from held messages with
  the same rules as `#221`: Show = all rows; Hide = read rows left out
  (`!stillToDo`, root exempt per (2) when summarized); Collapse = maximal
  runs of ≥2 read rows become one `.unpack-read-run` line with the exact
  `readRunText` words, given a bridge piece plus a short horizontal stub so
  the grey text hangs off the spine ("a little connector off the bridge").
  The newest row of a block never joins a run, as in `applyReadRuns`. Tapping
  a run line opens that run for that thread (per-thread opened set, forgotten
  on mode change, like `openedReadRuns`).
- **Lazy loading / performance**: blocks render first from
  `channelCanon.messages` (often already complete after reading All). A block
  whose held count is below the summary's exact count (or with no cover
  `viewKey("thread", id)`) shows "Loading thread…" and an
  `IntersectionObserver` (root `#side-scroll`, 400 px margin) queues a read
  of `timeline?view=thread&thread=<id>` folded by `foldTimelinePage(...,
  "thread", id)`, at most 3 in flight, only while unpacked and the list is
  on screen. Blocks are rebuilt only for threads whose data changed
  (signature per thread: ids + read mode + opened runs), so a poll redraws
  nothing. Measured and reported in the handoff: render time and row count
  on a synthetic 40-thread channel in the browser test.
- **Scope guards**: off = byte-identical cards (negative control). Search /
  Unread-only filtering hides a card's block with its card. Empty messages
  stay hidden via `glom`. Deleted/removed messages never resurrect: blocks
  read only from the canon.

## (2) root-under-summary

- `cardIsSummarized(choice)` = `sideCardRootText(choice) !== ""`.
- **Drill-in** (both columns): when the heading card is summarized, the root
  row is (a) always kept under Hide read (extend `readFilterKeeps` for the
  open thread's root when summarized), (b) never collapsed (already true),
  and (c) drawn as the card's first child: `data-root-child`, bridge piece,
  indent as a gathered reply. When the card is the raw root, nothing changes
  — no second copy, no indent (negative control).
- **Unpacked blocks**: when summarized, block starts with the root as first
  bridged child (even if read, even under Hide); when raw, block starts with
  the replies — the card already is the root, so no duplicate row.

## Tests

Page tests first (fail before the change): fake-DOM suite
(`vibe-talk/tests/js/voice_page.test.mjs`) for toggle persistence, keymap
entry, block contents/order, read-mode runs, root-child rules and negative
controls (toggle off unchanged; raw-root cards not duplicated; Hide read
hides read replies under every unpacked thread while keeping summarized
roots). Chromium walk (`vibe-talk/tests/threads_unpack_browser.py`, fixture
of `two_column_browser.py`) at 1280x800 and 393x852, dark and light, toggle
on/off, collapsed read runs, with screenshots.

## Addendum, later on 2026-10-21: `#407 unpack-row-parity`

The owner tried (1) and found the unpacked rows outside the facilities of every other list: no hover
highlight, no keys on the selected row, `:` doing nothing, and asked for shared code rather than a
separate implementation. What changed from the design above:

- The selection (`paneRows`, `rowOf`) no longer leaves `data-unpack-thread` rows out, so the mouse,
  ↓/↑/j/k, `n` and every key about one message reach them through `sidePane`, as any list's rows.
  A kept selection records whether it is on a card, since a card and its root's row can share an id.
- Blocks are built in the right column (the column `sidePane` acts in), with its read state caught
  up with the store's for the messages drawn, so a key and a row's own button act in one column.
- A block's rows are dressed by `dressChannelRow` (extracted from `renderChannelRows`), filtered by
  `readFilterKeeps(thread)` under Hide read, and folded by `readRunPlan` (extracted from
  `applyReadRuns`) under Collapse read: the `.unpack-read-run` line and its own opened-runs map are
  gone; a block's run is the ordinary run line, opened as in its thread (`openedReadRuns` under the
  thread's `viewKey`).
- `;` and `:` fold the unpacked rows (`visibleFolds` reads the cards' list in the right column); `→`
  on an unpacked row goes into its thread; Through here covers the rows above it in its own thread.
