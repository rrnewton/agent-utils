---
title: 'vibe-talk-search-whole-threads-toggle: Whole threads as a fixed-width toggle icon on the search bar'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- search
- ui
created_at: 2026-11-18T10:47:28-05:00
updated_at: 2026-11-18T10:47:28-05:00
---

# Description

[watermelon-full-preview] Owner (thread 190, verbatim): "Did we implement a button for "Whole thread search hits". I.e. in thread list view or in two column view or even in All view... search is a filter but threads are all-in or all-out. I find that even before I do more #labels on the messages I want to find all the threads related to topic X. They all mention the word X. But then I want to view those threads in the entirety and interact with them while leaving the filter on. I propose we make it a fixed width toggle icon on search bar with the others. It could be a greyed out thread emoji and clicking it to toggle on could make it a highlighted colored circle AROUND the threads emoji, the tool tip text can say "Whole threads: if any msg matches". Our other icons don't consistently have tool tip hovers and they should. Like the pinned threads button or the hyperlinks one."

Context: #1825 vibe-talk-hashtag-labels added an "Entire threads" chip in the search results area (the bucket row under the search bar, seen with `label:` searches). Check whether it covers plain word searches. Owner-priority, deployed on landing.

# Design

One control, on the search bar: move the #1825 "Entire threads" chip out of the results-area bucket row into `#search-float` beside the other fixed-width toggles (author, Links, Pinned, the glass), keeping the existing element id `search-entire-threads` so there is exactly one control and existing wiring/tests keep one home. Off: the thread emoji (🧵) greyed. On: the same emoji with a highlighted coloured circle around it (the pressed-toggle dress the bar's other filters wear, from `aria-pressed`). Tooltip text exactly "Whole threads: if any msg matches" as both `title` (hover) and `aria-label` (accessible name); state in `aria-pressed`.

Every search-bar icon (author, Links, Pinned, Whole threads, the glass) carries both a `title` tooltip and an accessible label (`aria-label`, or its text where it has words). No second control: the bucket-row chip is removed; the bucket row keeps only the #1797 scope buckets.

Behaviour: unchanged matching core (`setEntireThreads`/`entireThreadHits` in the search-rows owner, which already matches the group-expanded `label:` query and plain words alike through the one parsed query), extended so a thread-list card stands when its thread holds any hit (by the card's `data-context-id`, as the reply-hit reveal already does), in thread list, two-column (side cards refilter through the same pass) and All views. While on, any thread with a matching message shows in its entirety and the filter stays on while the reader reads and replies in those threads: no interaction resets it; only the toggle itself, or closing the search bar (which clears every bar filter, as Pinned/Links already do), turns it off.

# Acceptance Criteria

- Toggle icon on the search bar beside the others: fixed width, greyed thread emoji off, highlighted coloured circle around the emoji on, tooltip "Whole threads: if any msg matches" (`title` + `aria-label`), `aria-pressed` state; the old results-area chip is gone (one control).
- Tooltips (`title` plus an accessible label) on every search-bar icon: author, Links, Pinned, Whole threads, the glass.
- Works in thread list, two-column and All views, for word AND `label:` searches: any thread with a matching message shows in its entirety; the filter stays on while reading/replying in those threads (no reset on interaction).
- Browser test through the real page/search route on a test instance covering word and label searches in all three views, plus shots of the toggle off/on with tooltips, phone and desk, light and dark, keyboard open (real `visualViewport` shrink) in one phone shot, Discuss half-pane open in the phone shots.
- `make validate` green on the final tree via the gate (owner-priority), landed to main; coordb deploys.

# Notes

Prior work: #1825 vibe-talk-hashtag-labels (landed d1a87ebb) — `Message.labels`, `label:<slug>` search, and the Entire-threads chip beside the #1797 buckets (`setEntireThreads` in `web/src/view/channel/searchrows.ts`, chip in the bucket row in `web/voice.html`, wired in `web/src/view/channel/listcontrols.ts`). This issue moves and completes that control per the owner request above; the matching core is reused, not duplicated.
