# thread-list-animation: a calmer Threads view — proposal

Date: 2026-10-22. Status: **proposal only, nothing implemented.** The owner asked for a plan before
any code: "This is a tough one and I don't want to mess it up."

The owner's request, in short:

1. In **Unread only**, marking the last unread message of a thread read makes the whole card vanish,
   and the screen fills the space at once from the thread above. He would rather see the gap for a
   moment and then watch it close.
2. Threads are ordered by latest reply, so a thread that gets a reply jumps from somewhere above to
   the foot of the list. With **Unpack msgs** (`#404 threads-unpack-all`) the jump is especially
   disruptive. He suggested two remedies. One: treat the move as a removal (the others move up to make room
   at the foot, and the thread slides or fades in there). Or, **his preference**: the card packs up
   (its messages go into it), flies down *behind* the other cards, and unpacks at the foot with the
   new message. This has to work whether the thread is on screen or far up in the scrollback, without
   spending time animating anything off screen: "The stage for this play is just what's on screen."
   This document uses **stage** in that sense: the scroller's visible rectangle.
3. The one animation he notices today, and likes, is the drill-in: the thread's card flies up to the
   top of the screen.

## Summary

- **Everything asked for is feasible**, on Chrome desktop, the Android Chrome PWA and iOS Safari,
  with the plain Web Animations API (`Element.animate`), transforms and opacity, and one `z-index`.
  A throwaway prototype shows it in headless Chromium 149 (section 3). That includes the
  owner's preferred **pack, fly behind, unpack**, the **gap that holds and then closes**, and
  **clamping to the stage** for threads that come from, or go to, off screen.
- **The core idea:** the list's layout and the reader's scroll position settle *at once*, exactly as
  today, and the keepers of the reader's place (`#280 scroll-stability`, `keepingCardPlace`) still
  decide where the reader ends up (section 4.6 lists the few changes they need). Motion is a
  decoration painted over that settled layout: each element on the stage briefly gets a transform
  that draws it where it *was*, and then lets go (FLIP). That is how the drill-in already works ("the
  thread is already at its place when it fades in"). It is the only approach that works on all three
  browsers without fighting scroll anchoring.
- **Not recommended as the base:** the View Transitions API, or animating `height` /
  `grid-template-rows`. View Transitions is well supported now, but it draws the page through an
  overlay that takes the pointer for the length of the transition. It also escapes the scroller's
  clipping without a newer Chromium-only feature, and the next live update cancels it. Height
  animation means layout every frame and rewriting the scroll position every frame, and it can't
  animate the freshly rebuilt nodes this page draws.
- **Recommendation:** first fix two scroll-anchor jumps that exist with or without animation
  (section 4.6). Then build a small motion layer under `renderSideCards` and ship it in three steps:
  (1) the gap-hold-close for Unread only, with the engine, reduced motion and tests; (2) fly-behind
  for packed cards; (3) pack and unpack for Unpack msgs, with removal-plus-slide-in as the fallback
  when several threads move at once. Each step is useful on its own and can be switched off by one
  stylesheet variable.
- **Questions for the owner** are at the end. The main ones: how long the gap should hold, whether a
  card from far above should be seen entering from the top edge, what to do while an inline reply is
  open in the thread that moves, and what reduced motion should show.

## 1. How the Threads view is drawn today, and where the jumps come from

All references are to `vibe-talk/web/voice.js` and `vibe-talk/web/voice.css`.

### The one renderer

- The Threads view (Cmd+3, `#281 view-keys`) is the right column's card list drawn across the window
  (`threadListShown`). It is the same list as two columns' right column. The scroller is
  `#side-scroll`, and the list is `ul#side-cards`, a one-column CSS grid with a `0.45rem` gap
  (`.side-cards`). Cards are opaque (`background: var(--panel)`), with the thread's hue on their left
  edge (`.side-card-row`).
- `renderSideCards()` builds a signature of everything it would draw: each card's id, text, age,
  reply and unread counts, each unpacked block's `unpackSignature`, and Main's rows (`#419
  threads-view-main-messages`). It does nothing if the signature is unchanged. **If anything changed,
  it rebuilds every node:** `list.replaceChildren(...)` with fresh `<li>`s from `sideCard`, the
  blocks from `unpackBlock` (`#404 threads-unpack-all`, `#407 unpack-row-parity`) and Main's runs. The rebuild is
  wrapped in `keepingCardPlace`, which reads the reader's place first (`cardAnchor`) and puts it
  back afterwards (`placeCardAnchor`).
- **Newest activity is at the foot** (`#261 two-column-connectors`): `threadChoices()` is newest
  first and is reversed. A reader at the foot follows it (`followFoot`, with `#338
  column-overscroll`'s room past the foot at a desk). A reader up the list keeps the card they were
  on at its height: the highlighted card (`#277 thread-card-focus`), else the selected one, else the
  first card showing. `cardAnchor` only ever anchors on **cards**. With Unpack msgs, a reader looking
  at a long block with no card on screen has no anchor at all (null), so nothing holds their place.
- A `ResizeObserver` on the scroller's children (`footKeeper`) also follows the foot, and holds a
  reader in `#338 column-overscroll`'s room past it, whenever the content changes size. It runs
  after layout and before paint, so it can write `scrollTop` after everything else in the task.
- **Unread only** (`#259 thread-list-unread-filter`) is a filter pass, `filterSideCards()`. A card
  without `data-unread` gets `row.hidden = true`, and its block's rows follow it. Main's rows use
  `rowIsRead`.

### Jump 1: a card leaves Unread only

Marking the last unread message read changes that card's unread chip. The signature changes, the
list is rebuilt, and `filterSideCards` hides the card and its block, all in one task. Then
`placeCardAnchor` runs:

- **Reader at the foot (the usual case):** `followFoot` keeps the foot where it is. Everything
  *above* the vanished card drops by its height in the same frame. That is exactly "the display
  immediately paints to fill in some other thread above it".
- **Reader up the list, anchored on a card above the one that left:** everything below the gap moves
  up in one frame.
- **At a desk, reader in the room past the foot** (`#338 column-overscroll`): the page holds the
  scroll position there instead of following the foot, so the items *below* the gap move up. Or
  `fitOverscroll` closes the room in the same change, and the reader is moved to the new foot.

### Jump 2: a thread moves to the foot

A reply changes the thread's latest time. On the next rebuild its card and block are drawn last. If
the moving thread X has height `hX` before and `hX'` after (its new reply included), and the reader
is at the foot:

- Every element that was **below** X moves up on screen by **`hX'`**, the whole height of the
  arriving thread. This holds whether X was on screen or far up the scrollback. With Unpack msgs,
  `hX'` is a whole thread's messages, so the entire screen can be replaced in one frame. This is the
  "especially visually disruptive" case.
- Every element **above** X moves up by `hX' − hX` (typically the new reply's height).
- X disappears from where it was and appears at the foot.

If the reader is up the list with a card on screen, the anchor keeps the visible cards still, and a
thread moving from above to below them changes nothing on screen. **There are two hidden extra
jumps here:**

- `cardAnchor` falls back to *the first card showing*. If that card is the one that moves,
  `placeCardAnchor` finds it at the foot and scrolls the reader all the way down after it.
- With Unpack msgs and no card on screen (a long block filling it), there is no anchor at all, so a
  thread leaving from above moves the whole view by its height.

Section 4.6 fixes both: anchor on any item, never on a moving thread.

### Why CSS transitions can't simply be switched on

Every rebuild makes new elements, so a transition has no previous style to start from. The stylesheet
already says this in its `done-marking` section ("ANIMATIONS, NOT TRANSITIONS ... the row drawn read
is a new element"). Any motion has to be keyed by identity (thread id, message id) across rebuilds,
not by element. It also has to survive the *next* rebuild, which may come mid-animation, because a
card's age text alone changes every minute.

### The drill-in the owner likes (`#298 thread-card-header`)

`openThreadFromList` / `selectSideThread` measure the card's place in the list (`cardFlightStart`,
only when the card is on screen) before switching views. After the switch, `flyThreadCard`:

- reads the duration from the stylesheet: `--card-flight: 220ms` on `#screen-main`, which is `0ms`
  under `prefers-reduced-motion` (`cardFlightMs`). The script never asks a media query;
- measures the card at the top of the thread, writes the difference as `--flight-x` / `--flight-y`,
  and sets `data-flying` on the card and `data-arriving` on the pane;
- lets CSS do the motion: `@keyframes card-flight` runs *from* `translate(var(--flight-x),
  var(--flight-y))` to its place, with `cubic-bezier(0.2, 0, 0, 1)`; `position: relative; z-index: 3`
  ("over the list it flies across"); and the thread's pane fades in after a 60 ms delay
  (`thread-arrive`);
- cleans up on `animationend` for each part, with a timer (`ms + 100`) in case the browser never says
  the animation ended (`endCardFlight`). A new flight first ends the previous one.

That is a FLIP over a settled layout, with only transform and opacity animated, nothing laid out
again, and the duration owned by the stylesheet. The proposal keeps every one of those properties and
reuses the easing curve, so the list's motion feels like the drill-in. Two things change: the
multi-step list motion uses `Element.animate` instead of CSS keyframes (section 2.1), and the
fly-behind card goes **under** the list (`z-index: -1`) where the drill-in card goes over it.

## 2. Techniques and how portable they are

Target browsers: Chrome on the desktop, Chrome on Android as an installed PWA, possibly Safari on
iOS. Version numbers are from my knowledge as of mid-2026 and should be rechecked against current
compatibility tables before implementation. None of the recommended pieces is recent.

### 2.1 FLIP with the Web Animations API: recommended

`Element.animate(keyframes, {duration})`, `Animation.finished`, `pause()`, `currentTime`,
`finish()`, `document.getAnimations()`. All are in every current engine (Chromium since about 84 for
the full set, Safari since 13.1 / 14, Firefox since 75).

- **First, Last, Invert, Play:** measure where things are, change the DOM, measure again, and animate
  `transform: translateY(old − new)` → `none`. Measuring with `getBoundingClientRect` after the
  mutation, in the same task, never paints the intermediate state.
- **Multi-step keyframes:** one animation per element can hold, move, hold and move again (keyframe
  `offset`s), with per-segment easing. That is how every element shares a single timeline across
  pack → fly → unpack, and how a gap holds before it closes.
- **Compositor-only:** transform and opacity run off the main thread in Chromium (desktop and
  Android) and in WebKit. That matters on a phone, where this 49 000-line page's rebuild already
  costs its frame.
- **Deterministic for tests:** pause every animation and set `currentTime` to see any frame exactly
  (section 7).
- **Keyed across rebuilds:** measure by `data-id` (cards) and message id (rows), not by element, so
  the fresh nodes from `replaceChildren` animate from where the old ones were.

### 2.2 The View Transitions API (same-document): not as the base

Support: Chromium 111+ (desktop and Android), Safari 18+, Firefox 144+. `view-transition-class` and
transition types: Chromium 125+, Safari 18.2+. Nested transition groups
(`view-transition-group`), which let a group be clipped by an ancestor such as a scroller: recent
Chromium only (about 140). I would not rely on them in Safari without checking. Element-scoped
transitions (`element.startViewTransition`) were still experimental as of mid-2026.

It is attractive on paper: name each card, call `document.startViewTransition(update)`, and the
browser morphs old to new, size changes included. In this page:

- **The page is drawn through the transition's overlay for the duration.** The new state is shown
  as live images (`::view-transition-new` is live, so typing stays visible), but the pseudo-elements
  cover the page and take the pointer. An inline reply open in the list (`#410
  inline-reply-editor`) keeps its keyboard focus but can't be clicked into, and it moves as an image
  in the top layer. That is the opposite of "keep an open editor stable".
- **It escapes the scroller.** Snapshots are drawn in the top layer, so a card flying down the list
  draws over the heading chips and the message box, unless nested groups are used (Chromium-only).
- **The next update cancels it.** Starting a transition while one runs skips the running one to its
  end. Live replies arrive whenever they like, so every overlap would be a snap.
- **Every moving element needs a unique name and is captured separately.** With Unpack msgs, that
  is every card and every row, hundreds of captures, for a list where only a few things are on stage.
- **Fly-behind is possible** (`z-index` on `::view-transition-group(...)`), but only if every
  sibling it passes behind is named too.

It may suit a later whole-view navigation, but for a live, scrolling, editable list it is the wrong
tool.

### 2.3 CSS transitions on `height` / `grid-template-rows`: not for this

- `height: auto` only transitions with `interpolate-size: allow-keywords` or `calc-size()`
  (Chromium 129+; not in Safari or Firefox as of mid-2026). The `grid-template-rows: 0fr ↔ 1fr`
  trick is portable (Chromium 107+, Safari 16+, Firefox 66+), and `@starting-style` (Chromium 117+,
  Safari 17.5+, Firefox 129+) lets a new element transition in.
- But these animate **layout**. Every frame lays out the whole list below the change, which costs
  more as Unpack msgs grows. More importantly, **a gap closing above the reader moves everything
  under it** unless the scroll position is rewritten every frame. Native scroll anchoring
  (`overflow-anchor`) does that in Chromium and Firefox but is not in Safari as of my knowledge.
  Writing `scrollTop` every frame from script stops momentum scrolling on iOS and fights the page's
  own keepers.
- With FLIP the gap is *drawn*, not laid out: the layout is already final, and the elements on one
  side are translated to leave the gap. Closing it is compositor work with no layout and no scroll
  writes.

### 2.4 `z-index` layering for "fly behind": portable

- `#side-cards` becomes its own stacking context (`isolation: isolate`). An element in it with
  `z-index: -1` paints **after** the list's own background but **before** every sibling's background
  (CSS 2 painting order: negative z-index layers come before in-flow block backgrounds). Grid items
  honour `z-index` without being positioned, so the flying card needs no `position` change.
  Transformed siblings paint in the positioned layer, which is still above it.
- The scroller's background (outside the list) stays behind everything, so the flying card is
  visible in the 0.45 rem gaps between cards and beside the indented unpacked rows, and hidden behind
  the cards themselves. That is literally "behind the other cards".
- **Read rows are translucent.** They are drawn at `--row-opacity: 0.42` (noise rows 0.32) and in
  greyscale, so the flying card will show *through* every read row it passes, not only in the gaps.
  The prototype has no dimmed rows, so this is unseen (question 7).
- A thread's own rows travel at `z-index: -2`, under their own card, so they pack *into* it and come
  out *from under* it.
- `isolation` is in every engine (Chromium 41, Safari 8, Firefox 36). The open inline editor's box
  is drawn outside the list (`replyMountFor` → the list's parent), so the new stacking context
  doesn't change its layering.

### 2.5 Reduced motion

Follow the page's existing rule: the stylesheet owns the durations and sets them to `0ms` under
`@media (prefers-reduced-motion: reduce)`. The script reads them, as `cardFlightMs` reads
`--card-flight`, and with 0 it does exactly what the page does today. Proposed variables, on
`#screen-main` next to `--card-flight`:

| Variable | Proposed | Meaning |
|---|---|---|
| `--list-gap-hold` | 260ms | how long a departed card's gap stays open (the card's 150 ms fade-out runs inside it) |
| `--list-close` | 280ms | a gap closing, whatever its size (the displacement is clamped to the stage) |
| `--list-pack` | 170ms | a thread's rows going into its card |
| `--list-fly-min` / `--list-fly-max` | 280ms / 500ms | the flight, scaled by on-stage distance, clamped |
| `--list-unpack` | 240ms | rows coming out of the card at the foot; room opening |

Under reduced motion, nothing moves. **Proposed:** the thread that arrived at the foot is lit
briefly, so the reader can still see what changed. There is a precedent: a landed reply
(`data-landed`, `reply-landed`) is "lit and then simply gone" under reduced motion, its fade switched
off but its highlight kept for `REPLY_LANDED_MS`. The arrived card would do the same. Owner's call
(question 5).

## 3. The prototype

A throwaway, standalone HTML page (not in `vibe-talk/web`, and not committed; it, its driver, frame
strips and recordings go with this work's handoff) reproduces the list's
shape: an opaque one-column grid of cards with a 7 px gap, indented rows under each card, newest at
the foot, a full `replaceChildren` rebuild on every change, and a `keepingCardPlace`-style keeper.
On top of that it has a ~250-line motion layer. It was driven at 393 × 852 by Playwright in headless
Chromium 149, with frames taken by pausing every animation at a chosen `currentTime`, plus real-time
recordings.

What it showed:

- **Gap, hold, close** (Unread only, reader at the foot): the card fades out in its first 150 ms
  and leaves the gap open, 260 ms in all, the fade included. Then the cards above slide down over
  280 ms, and the next thread above slides in from under the top edge. 540 ms in all.
- **Pack, fly behind, unpack**, source on screen: the rows fold up into the card (170 ms). The card
  travels to the foot behind the cards below it, showing as a hue edge and a shadow in the gaps and
  beside indented rows. Meanwhile the others move up to make room. Then the rows, with the new reply,
  come out from under the card (240 ms). About 800 ms in all.
- **Source far above the screen:** no pack (unseen), and the card enters from just over the top edge
  and flies down behind (760 ms).
- **Reader up the list, source on screen:** the card packs and flies out through the bottom edge.
  The reader's anchor card does not move, and the off-screen unpack costs nothing (680 ms).
- **Removal plus slide-in:** the old place closes, then room opens at the foot and the thread slides
  up 18 px and fades in (520 ms).
- **Checks** (scripted, all passing):
  - At `currentTime = 0`, every *card* on stage other than the moving one is where it was before the
    change (no first-frame jump). Rows and ghosts were not checked this way.
  - The last frame equals the settled layout.
  - The "today" control moves four on-stage cards in one frame.
  - **Behind:** at 20 ms steps through each flight, at every sample point where a sibling overlaps
    the flying card (2 000 to 3 000 per scenario, inset from the boxes' rounded corners),
    `elementFromPoint` returns the sibling, never the flying card.
  - **On stage:** the flying card is never more than its own height outside the stage.
  - Under reduced motion no transform animation is created and the arrival is still marked.
  - A live reply that arrives mid-flight is queued and then played.
- **CSS anchor positioning** (how `#410 inline-reply-editor` draws its box): in Chromium 149 an anchored box
  follows a transform on its anchor row, both a static transform and a running animation. This was
  not checked in WebKit, so the design never transforms the editor's row anyway (section 4.7).

Limits: it is Chromium only, has no real data and no read modes, so it judged feasibility and feel,
not the page. In particular it has none of these, which the page has and section 4 deals with:
`#338 column-overscroll`'s room past the foot, the `footKeeper` resize observer, row margins
(seams, bookmarks, the inline reply's room), translucent read rows, lazily read blocks, and native
scroll anchoring (its stage sets `overflow-anchor: none`). Its keyframe stages were found by
rendering each intermediate state synchronously; section 4.2 derives them from measurements instead.

## 4. Design

### 4.1 Shape: one motion layer under `renderSideCards`

A small module (call it `listMotion`) in `voice.js`, with three entry points:

1. **`capture()`**, called inside `keepingCardPlace` before the rebuild, and only when a motion
   *intent* is pending (below). It records, for every shown item of `#side-cards` (card, unpacked
   row, Main row, loading line): its key, its order and its screen rectangle, plus the stage and the
   scroll position. It reads layout once, with no writes in between.
2. The rebuild and the keeper run as today (with the changes in 4.6).
3. **`play()`**, called at the end of `renderSideCards` after `placeInlineReply`. It measures again,
   diffs by key (departed, arrived, moved, stayed), plans, and starts the animations.
4. **The last word on scroll.** Something can still write `scrollTop` after `play()`, before the
   paint: `footKeeper`'s resize observer runs after layout, and `dockReply` scrolls to show its box.
   Any such write would shift everything, animated or not, by the same amount, which is a first-frame
   jump. So `play()` notes `scrollTop`. Before the first frame is painted (a resize observer
   registered after `footKeeper`, which runs after it in the same pass), the layer compares it again
   and re-bases every animation by the difference. The browser test's "frame 0 equals before"
   check (section 7) exists to catch any writer this misses.

**Intents, not inference.** Motion runs only when a caller says why the list is changing:

- `"read"`: the reader marked something read or Done (key `e`, the Done button, mark-read paths).
- `"live"`: a message folded in from the stream (`foldLiveMessage`'s path to the redraw).

Everything else stays instant: typing a search, Unread only or Unpack msgs pressed, a channel switch,
a reload, the threads sort order (`#426 threads-sort-toggle`, in progress elsewhere and not yet on
`main`), opening or closing the list. An allow-list is the safe default: a change nobody thought
about never animates by surprise. **It also means some moves and departures still jump**, and should
be named as such rather than discovered:

- the thread list's periodic re-read (`refreshThreadDirectory`) reordering threads;
- an unpacked block's lazy read bringing newer replies (4.8);
- reads synced from another device or from upstream, which empty a card's unread count without the
  reader doing anything.

Each can be given an intent later if it turns out to matter.

### 4.2 The in-between layouts: derived from two measurements, not rebuilt

The multi-step variants need positions for layouts that never exist (X packed in place, X packed at
the foot). The prototype rendered them. The page should not, and it **cannot simply add up heights
either**. Rows carry their own margins (a seam's `--seam-room`, the bookmark's room, the inline
reply's `--inline-reply-room`). Hidden rows, read-run members and search-hidden rows sit among the
shown ones. And Main's runs (`#419 threads-view-main-messages`) regroup when a card's time changes,
so rows that did not move can change height or key.

What it can do is derive the in-between stages from the two layouts it **does** measure, S0 (before)
and the final one, using only X's measured spans. A span is from X's card top to the top of the
next shown item, so it includes every margin.

- **Which side moved** is read from the measurements, not from the keeper's rule: compare a
  stationary item above X and one below X between the two snapshots. That covers the reader at the
  foot, an anchored reader, the `#338 column-overscroll` room past the foot, and whatever
  `footKeeper` did, with no special cases.
- **X packed in place** is S0 with the items on the moving side shifted by X's block span.
- **X packed at the foot** is the final layout with the items on the moving side shifted by X's final
  block span.
- **Items whose height or key differs between S0 and the final layout** (a regrouped Main run, a
  row that grew because a reply was glommed into it) are not given in-between stages. They go
  straight from S0 to their final place, or fade if their key is new.
- **If that would be most of the stage** (for example many Main rows between the cards regrouping),
  fall back to variant A, which needs only the two real measurements.

Only the derivation from (two snapshots, X's spans, the moving side) is a pure function. That part is
unit-testable in node (section 7).

### 4.3 Scenario 1: the gap appears, holds, then closes

Applies to: a card leaving Unread only because of a `"read"` intent. The same mechanism later covers
a Main row leaving Unread only, and an unpacked row leaving under Hide read.

1. `capture()`; rebuild; the card and its block are hidden; the keeper places the reader as today.
2. `play()`: each item on stage at either end is offset by `translateY(old top − new top)`. With
   the reader at the foot, the items above the gap moved *down* in layout, so they get `−h` (drawn
   back up where they were) and the items below get 0. Anchored above the gap, it is the reverse.
   Each gets keyframes: hold at the old place for `--list-gap-hold`, then ease to its place over
   `--list-close`.
3. **Clamp:** under Unpack msgs a departing card and its block can be taller than the stage. The
   offset is capped at the stage's height. Every frame is still continuous, because items that
   would start further away than that are off stage anyway, so what is seen is at most a screenful
   of gap closing in `--list-close`.
4. In the gap: a **ghost**, a clone of the departing card (and its shown block), where it was,
   fading out over 150 ms (`fill: forwards`) and removed at the end. **Not in `#side-cards`:** many
   walks read that list's children (`filterSideCards` counting cards for "No unread threads.",
   `cardAnchor`, `placeCardAnchor`, the keys' row lookup, the column links, `lastColumnCard`), and a
   clone would copy `data-card`, `data-id`, `data-unread` and a duplicate anchor name. Ghosts go in
   an overlay layer beside the list (in `#side-list`, positioned), with those attributes and ids
   stripped, `aria-hidden`, `inert` and `pointer-events: none`. The real card is already hidden, so
   the keys, the counts, the screen reader and "No unread threads." are all final at once.
5. The row's own Done flash (`data-done-flash`) is on the departing row, so it is in the ghost
   clone and keeps playing there. Whether to let the flash finish (250 ms) before the fade starts is
   a feel question (question 1).
6. **Where the read happens.** The departure is on stage when the read is made in the list itself
   (`e` on an unpacked row) or in the other column (two columns: Main or All on the left, the cards
   on the right). When the read is made *inside a thread* opened from the list, the card is not on
   screen when it departs, and coming back finds it gone. Proposed: nothing is replayed then. The list
   comes back as it is now, as it does today (question 1).

The gap is visual only: **nothing in layout waits**, so every existing keeper and test that reads
positions *after the animation* still sees the same numbers. Code that reads positions *during* it
sees transformed boxes; see 4.6.

### 4.4 Scenario 2, variant A: removal, then slide-in at the foot

Stages: S0 now; S1 X removed (its old place closed); S2 final (X' at the foot).

- Siblings: S0 → S1 over `--list-close` (only if X's old place is on stage; otherwise 0 ms), then
  S1 → S2 over `--list-unpack`. With the reader at the foot, S1 → S2 is "everyone moves up to make
  room".
- X: a ghost fades at its old place (if seen). The real X' at the foot holds at opacity 0 until S1,
  then rises 18 px and fades in.

This is the simpler variant and it reads well. **Proposed role:** the fallback whenever the
preferred variant would be too busy: several threads moving in one coalesced batch, or a moving
thread taller than the stage.

### 4.5 Scenario 2, variant B (the owner's preference): pack, fly behind, unpack

Stages: S0 now; S1 X packed in place (its rows gone, the card stays); S2 X packed at the foot (moved,
no new reply yet); S3 final (X' unpacked at the foot with its new reply).

| Phase | Duration | X | Siblings |
|---|---|---|---|
| pack | `--list-pack` if X's old place is on stage, else 0 | rows ease up into the card, `scaleY(0.6)`, fade to 0 | the block's space closes |
| fly | distance on stage × ~0.9 ms/px, clamped to `--list-fly-min..max`; 0 if neither end is on stage | the card travels at `z-index: -1` with a shadow | the card's old slot closes; card-sized room opens at the foot |
| unpack | `--list-unpack` if the foot is on stage, else 0 | rows (the new one included) come out from under the card at `z-index: -2` and fade in | the rest of the room opens |

The card is the **real element** of the settled layout, drawn where it was and moved home, so when
the animation ends there is nothing to swap. Its old rows ride along invisibly under it (keyed by
message id), and the new reply is simply a row with no "before". In packed view (Unpack msgs off),
there are no rows: pack and unpack are empty and the card just flies behind.

**Where it comes from and goes to (the stage):**

- Clamp both ends of the flight to the stage. A card that was above the screen starts **just over
  the top edge** (its bottom at the stage's top). A card whose destination is below the screen ends
  **just under the bottom edge**. The scroller clips it, so it slides in from under the top edge or
  out under the bottom edge. Nothing travels a single pixel off stage, and a thread from a week ago
  costs the same as one from a minute ago.
- Durations scale with *on-stage* distance and are clamped, so the worst case is about 500 ms of
  flight.

| Source | Destination (foot) | What is seen |
|---|---|---|
| on screen | on screen (reader at the foot) | pack, fly down behind, unpack; others move up |
| above the screen | on screen | enters from the top edge, flies behind, unpacks; others slide up by `hX'` (displacement capped at the stage height) |
| on screen | below the screen (reader up the list) | packs, the gap closes, flies out under the bottom edge; the anchor doesn't move |
| above | below | nothing on stage moves (with the anchor fix in 4.6); no animation, no wall-clock spent |
| (hidden by Unread only) | on screen | there is no source: arrival only (unpack at the foot, others move up) |

Optional (question 3): with the destination below the screen, add a quiet pointer that something
arrived at the foot. The column's existing jump-to-newest affordance may be enough.

### 4.6 Scroll anchoring and the reader's place

- **Unchanged:** `followFoot`, the `#338 column-overscroll` room past the foot, `footKeeper`,
  `placeOpenedCards`. They place the reader in the settled layout, and the motion is computed from
  where they put it (4.1, 4.2).
- **Changes to `cardAnchor`.** These are behaviour changes, worth making even without animation,
  and each is a commit of its own:
  1. Never anchor on a thread *that is moving*, or the reader is carried to the foot with it
     (section 1, Jump 2). Pass over moving threads and take the first stationary item on screen.
  2. Anchor on any shown item (unpacked rows and Main's rows too), not only on cards, so a reader
     looking at a long block keeps their place when a thread above them leaves.
  3. While an inline reply is open in the list, anchor on its row (4.7).
  4. The highlighted card (`#277 thread-card-focus`) is the reader's explicit choice. Question 4
     asks whether the reader should follow it to the foot or stay where they are.
- **Native scroll anchoring:** `#side-scroll` does not set `overflow-anchor`, so Chromium may try
  its own anchoring around the rebuild. The page's keeper sets `scrollTop` explicitly afterwards, so
  that wins. The motion layer measures only after the keeper has run, so the result is right either
  way. The proposal adds nothing that depends on native anchoring, which matters because Safari
  doesn't have it.
- **The reader scrolling mid-animation counts as a reader action** (4.9): `wheel`, `touchstart` and
  the scrolling keys on the scroller call `finish()`. Only items on stage were animated, so
  scrolling would otherwise bring in unanimated neighbours at their final places, next to displaced
  ones.
- **Code that measures during a transition.** `getBoundingClientRect` includes transforms. The
  room past the foot (`overscrollFit`, `noteOverscrollReader`), `atBottom` in the scroll handler,
  and the unpack `IntersectionObserver` would read displaced boxes for up to a second. Transformed
  boxes also count toward the scroller's scrollable overflow, so `scrollHeight` can grow and shrink
  for the duration. Mitigations, to be checked in step 1: `overflow: clip` on `#side-cards` (clipped
  overflow does not extend the scroll range, and the list's own box is already the settled layout);
  and these readers either wait for the transition (as the deferred render does) or call
  `finish()` first.

### 4.7 Inline reply (`#410 inline-reply-editor`): keep an open editor still

The editor is drawn by CSS anchor positioning under its row (`data-inline-reply`), outside the list.

- **While an editor is open, its row is the keeper's anchor** (a change to `cardAnchor`, 4.6: today
  the anchor is only ever a card, and the editor's row is the last row of its gathered block), so
  that row's screen position never changes and it gets no transform. Everything else moves around it. Chromium was seen to carry an
  anchored box along with a transformed anchor; WebKit was not checked. Never transforming that row
  makes the question moot, and a box sliding while the owner types would be wrong anyway.
- **If the thread with the open editor is the one that gets a reply,** proposed: it does **not**
  move while the editor is open. The new reply is drawn in its block where it is, and the thread
  takes its place at the foot (with the fly animation, if on stage) when the editor closes. That needs
  a small "held order" in `threadChoices` / the card order for that one thread. That is a behaviour
  change, not only motion. This is question 4, because it briefly breaks "ordered by latest reply".
- If the editor's thread departs under Unread only (the reader's own reply marks things read): the
  editor stays where it is, and the departure is shown (gap, hold, close) only after the editor
  closes, or not at all.

### 4.8 Unpack msgs (`#404 threads-unpack-all`), read modes, per-thread pack (in progress)

- Unpacked rows are keyed by the first message id of their glom group. A regroup
  (`combineMessages`) changes keys; such rows fade in rather than move. A new reply from the same
  author can instead be glommed into the last row: the key stays and the row grows. Rows below it
  still move smoothly (their positions are measured), but the grown row snaps to its new height.
  That is acceptable, or it can be revealed with a clip later.
- **Lazily read blocks.** A block whose replies the page doesn't hold is a "Loading thread…" line
  until its read, started as the block nears the screen (`watchUnpackBlock`), comes back. A thread
  arriving from far above may therefore unpack at the foot as a loading line, and its rows grow in
  a moment later with no intent attached, which is a jump at the foot. Proposed: a block read that
  completes for a thread that arrived in the last transition carries that transition's `"live"`
  intent, and its growth is played as the unpack phase.
- **Rows without a message** (the loading line, "Could not load this thread.", "Older messages are in
  the thread.") are keyed by thread and kind, so they move with their block.
- **Collapse read:** a read run is one row (its head) in the list, so it moves like any row. When the
  arriving reply reopens or splits a run, the changed rows just fade.
- **Hide read:** read rows are absent, so blocks are shorter; nothing special is needed. The row a
  reader marks read vanishing from a block is the same gap-hold-close case, in a later step.
- **Per-thread pack** (in progress elsewhere; no issue or slug yet): a thread the reader packed by
  hand has no rows to pack or unpack. It flies as a bare card and arrives packed. The animation's
  pack and unpack are drawing only and never change the reader's pack choice.
- **Threads sort order** (`#426 threads-sort-toggle`, in progress elsewhere): under "Thread created", replies
  don't reorder threads, so Scenario 2 never happens there. New threads arrive at the foot (arrival
  only).

### 4.9 Live updates arriving mid-animation

- **The renderer is held while something moves.** A `renderSideCards` that would rebuild during a
  transition is deferred, not run. So are the in-place passes that change a row's size without a
  rebuild (`dressUnpackedRows`, `markReplyMatch`, the `filterSideCards` path taken when nothing is
  rebuilt); or, where deferring one is wrong, it calls `finish()` first. The signature check makes the deferred call cheap, and one call
  at the end of the transition draws whatever accumulated (with its own motion, if a `"live"` intent
  is pending). This is necessary because a rebuild replaces every node and would kill the running
  animations in a single frame. The age text alone changes every minute.
- **Live updates are queued and coalesced.** If one thread moved, play it normally. If several did,
  play one variant-A transition for all of them (fade-ins at the foot, everyone else FLIPs), never a
  queue of flights. Worst-case latency is one transition (under a second).
- **A reader action never waits.** A command key, click, scroll (4.6), scroll-to-newest, search
  keystroke or filter change first calls `finish()`: every animation jumps to its end (which *is* the
  real layout), ghosts are removed, and any deferred render runs. Then the action proceeds. **Typing
  in a text field is not such an action:** keys in the inline reply or in Main's box (now in the
  Threads view too, `#419 threads-view-main-messages`) leave the motion alone. This mirrors
  `endCardFlight`, which ends a running flight before starting another.
- **A safety timer** (total + 100 ms) ends everything if the browser never resolves `finished`, as
  `flyThreadCard`'s timer does.
- `document.hidden` (a backgrounded PWA): no motion; apply instantly.

### 4.10 Two columns, the phone, cost

- Two columns share `renderSideCards`. Proposed: the same motion applies on the right column's cards
  (it is the same list), unless the owner wants it confined to the Threads view (question 6).
- Phone: the stage is small, so more cases fall under clamping, and fewer things move. All of it is
  transform and opacity.
- Cost: `capture()` and `play()` read one rectangle per shown item and plan in O(items). With 40
  unpacked threads (the `#404 threads-unpack-all` browser test's channel), I expect that to be well
  under the rebuild's own cost. This was not measured; step 1 measures it.
  The plan animates only items on stage at some point: a few dozen elements at most.

## 5. Recommendation and phasing

**Recommend the owner's variant B (pack, fly behind, unpack)** for a single moving thread, with
variant A as the automatic fallback for batches, all on the WAAPI FLIP layer above. The prototype
says it is feasible and legible. The card is visible behind the others only at the gaps and the
indented rows' margin, which reads as "it went behind" without competing with the cards in front.

| Step | What ships | Why in this order |
|---|---|---|
| 0 | the `cardAnchor` changes of 4.6 (no moving anchor, any item as anchor), each with a page test | fixes jumps that exist with or without animation; small and separately reviewable |
| 1 | `listMotion` (capture, play, the scroll re-base, finish, render hold, queue), the stylesheet durations and reduced-motion zeros, **gap-hold-close for Unread only**, the planner's node tests and one browser test | the smallest change that proves the engine on a reader-triggered case; no live-update races yet |
| 2 | **Fly behind for packed cards** (`"live"` intent), arrival-only for threads Unread only had hidden, coalesced batches as variant A, the open-editor rules (4.7) | adds live updates and z-layering, with no rows involved |
| 3 | **Pack and unpack under Unpack msgs** (stages derived as in 4.2), rows keyed by message id, read modes, lazily read blocks; then row departures under Hide read, and Main rows (`#419 threads-view-main-messages`) leaving Unread only | the most moving parts, on a proven base |

Each step can be turned off in the stylesheet (durations at 0) without a code change. That is also
the kill switch if a device misbehaves.

Not proposed: the View Transitions API; layout (height) animation; keyed node reuse in
`renderSideCards`. The last would let animations survive a rebuild and save work on every redraw, but
it is a large change to a central renderer. The render hold (4.9) gets the same effect for the length
of a transition, so keyed reuse can wait until something else wants it.

## 6. Risks

- **A first-frame jump if `capture()` is late.** If something else changes the list's layout or
  scroll earlier in the same task, before `capture()`, the "before" positions are not what was on
  screen. Mitigation: capture at the top of the intents' handlers when in doubt, and keep the browser
  test's "frame 0 equals before" check, which catches exactly this.
- **Deferred renders hiding state.** While a transition holds the renderer, a card's counts can lag
  by up to a second. Reader actions flush it at once, so only passive viewing sees the lag.
- **Interaction with the Done flash, the lit card, the selection ring, and the column links
  (`#261 two-column-connectors`, redrawn per frame during the drill-in).** The links follow cards: during list motion they
  should be redrawn per frame like `flyThreadCard`'s `follow`, or hidden for the duration. That is
  decided in step 2 with screenshots.
- **Visual noise.** Several live replies in quick succession on a busy channel could keep the list
  in motion. Coalescing bounds it to one transition at a time. A per-channel cap (e.g. no fly more
  often than every 2 s; otherwise plain variant A or instant) is cheap to add if needed.
- **WebKit unverified.** Nothing here is exotic, but nothing was run in Safari. A Playwright WebKit
  smoke run (if available on the build host) plus a look on the owner's phone is part of step 1.
- **Readers of layout during a transition** (4.6): the room past the foot, `atBottom`, the unpack
  observer and the scroll range all see transformed boxes. The `overflow: clip` mitigation is
  unproven until step 1 measures it in Chromium.
- **The in-between stages are an approximation** (4.2) where Main's runs regroup or rows change
  height. The fallback to variant A bounds how wrong it can look. The frame tests at the stage
  boundaries are what show whether it is good enough.
- **The fake-DOM suite has no `Element.animate`.** The motion layer must treat a missing `animate`
  like reduced motion (instant). Then every existing page test keeps its meaning, and motion is
  tested where it can be: in the planner (node) and in Chromium.

## 7. Test strategy

1. **The planner, in node** (`tests/js`): a pure function from the two measured snapshots (key,
   order, rectangle), X's spans and the stage to a plan (per key: keyframes, offsets, z-layer; total duration;
   which phases were dropped). Table-driven cases: source on, above or below the stage; destination on
   or below; reader at the foot, anchored, or in the room past the foot; items that change height
   between snapshots; Unread only arrival; clamped durations and displacements; batches and
   regrouping falling back to variant A; no motion for unknown intents.
2. **Chromium, deterministic frames** (a new `*_browser.py` on the existing fixture, e.g.
   `two_column_browser.py`'s API): perform the change, then `document.getAnimations()` → `pause()` and
   `currentTime = t` for chosen `t`, and read rects or take screenshots:
   - frame 0: every on-stage item, rows and ghosts included, at its pre-change top (no first-frame
     jump), also with the room past the foot open and with an inline reply open;
   - mid-hold: the space between the cards either side of the departed one is at least its height;
   - mid-flight: wherever a sibling overlaps the flying card, `document.elementFromPoint` returns the
     sibling, not the flier (it is *behind*), and the flier is within the stage ± its height
     (clamping). The prototype's driver already does both;
   - the last frame equals the settled layout, and the settled layout **and `scrollTop` equal a run
     with reduced motion**. The motion is decoration only, which also proves every keeper unchanged;
   - total duration ≤ the clamp.
3. **Reduced motion:** `reduced_motion="reduce"` → no transform animations, the same settled layout,
   and the arrival highlight present (if the owner wants it). The thread-card-header test's reduced
   path is the model.
4. **Interruptions:** a live reply mid-flight is queued and played after; a key press mid-flight
   finishes everything at once (positions equal the settled layout in the next frame), and so does a
   wheel turn; typing in the inline reply does not; a deferred rebuild runs exactly once.
5. **Real-time smoothness:** the rAF sampling `thread_card_header_browser.py` already uses for the
   drill-in (`window.__flight`), to check monotonic movement with no frame where a card is back at
   its old place after starting to move.
6. **For the owner's eye, not for pass/fail:** frame strips (seeked screenshots) and short
   recordings (`record_video_dir`) at desk and phone sizes, attached to each step's commit notes.

## 8. Questions for the owner

1. **The gap's hold.** About 260 ms (the card fading out in the first 150 ms of it), then 280 ms
   closing. Longer, shorter? Should the departing
   card fade out (150 ms) or simply vanish, leaving the gap? And after marking a thread's last
   message read *inside* the thread, should going back to the list replay the card leaving, or just
   show the list without it (proposed)?
2. **Which side closes the gap.** With the reader at the foot, the cards *above* slide down into it,
   because the foot is kept where it is. That is the only choice that doesn't push the reader off the
   foot. Is that what you pictured, or did you picture the cards below rising?
3. **From far above.** When the moving thread was off screen above, should its card visibly enter
   from the top edge and fly down behind (proposed), or just arrive at the foot (variant A's slide-in)?
   And when its destination is off screen below, is a quiet "arrived below" pointer wanted?
4. **The reader's own anchors.** (a) If the *highlighted* card's thread gets a reply, should the
   view follow it to the foot, or stay put and let it fly away? (b) While you're typing an inline
   reply in a thread that gets a new message, is it right that the thread **stays where it is** until
   you send or cancel, and only then goes to the foot?
5. **Reduced motion.** Nothing moves (as today). Should the arrived thread get a brief colour
   highlight instead, or nothing at all?
6. **Scope.** Threads view only, or the right column's cards in two columns too (the same list)?
   And should a Settings switch for motion exist separately from the system setting?
7. **How visible "behind" should be.** The card shows through the gaps, beside indented rows, and
   through read rows, which are drawn translucent. That is literally behind. With full-width
   unread cards on a phone it is mostly hidden. Is that the effect, or should the cards in front go
   slightly translucent (about 92%) during a flight so the silhouette always shows through? Or
   should it instead be hidden behind read rows too?
