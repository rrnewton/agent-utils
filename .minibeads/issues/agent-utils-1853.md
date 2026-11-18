---
title: 'vibe-talk-labels-dropdown-pinned-builtin: pinned threads button becomes a Labels drop-down with Pinned as built-in first label'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- labels
- ui
created_at: 2026-11-18T10:47:41-05:00
updated_at: 2026-11-18T10:47:41-05:00
---

# Description

[watermelon-full-preview] Owner (thread 190, verbatim): "I want the pinned threads button to evolve into a drop down soon for labels among which Pinned would be a built in first option."

This decides the earlier open question left by #1825 vibe-talk-hashtag-labels: Pinned BECOMES a built-in label, the first option, in the same label namespace as hashtag labels (#1825/#1835/#1836). Owner-priority gate, deployed on landing.

Today the search bar (`#search-float` in `vibe-talk/web/voice.html`) holds a single-purpose toggle, `#pinned-filter` ("Show only pinned messages", `aria-pressed`), wired in `vibe-talk/web/src/view/channel/filterchips.ts` to `setPinnedOnly` in `vibe-talk/web/src/view/channel/pins.ts`. Pins live server-side in the `pins` + `pin_revisions` tables (`vibe-talk/src/store/sqlite.rs`, bounded at `MAX_PINS_PER_CHANNEL = 100` in `vibe-talk/src/store/mod.rs`), with snapshots so a pinned message draws even when older than the loaded history. Hashtag labels are a separate, newer system: derived server-side from `Message.content` at serve time, exposed as `Message.labels`, filtered with the `label:<slug>` operator in the shared search language (`vibe-talk/src/search_query.rs` and `vibe-talk/web/src/core/search/language.ts`), counted by `GET /api/v1/labels` (`vibe-talk/src/http/api.rs`, `vibe-talk/src/ops.rs`), grouped in Settings > Labels (`vibe-talk/web/src/view/settings/labels.ts`), and expanded through label groups when searching (`expand_labels` / `expandLabelQuery`).

DESIGN NOTE FIRST: no code until coordb approves the design note sent by mu-labelsdrop. The note turns the pinned-threads button into a Labels drop-down, covers the Pinned-to-pins-storage mapping, keyboard and screen-reader behaviour, mobile layout, count badges, and the interaction with Settings > Labels grouping and the Whole-threads toggle.

# Design

Design note sent to coordb for approval before any code (this section records what was proposed; the approved text governs).

- Control: replace the `#pinned-filter` toggle in the search bar with a Labels drop-down button in the same slot (same visibility rule as today: shown only while the search bar is open on the Discord/channel view, per `vibe-talk/web/src/view/shell/controlbar.ts`). The button keeps the pin glyph while Pinned is the familiar entry, adds the word "Labels" and a chevron, and names the active filter in its accessible label.
- List: "All messages" (clears the filter) first as the way out, then Pinned as the built-in first label (gold treatment stays Pinned-only, per #1825 and `ai_docs/plans/2026-11-07-voice-overview/README.md` §8.1), then hashtag labels and label groups most-used first (count descending, A–Z tiebreak, the Settings > Labels default sort), groups as one row under their display name with the `(G)` marker. A filter box at the top of the panel narrows long lists by substring, matching the Settings > Labels filter.
- Selection: single-select in v1. Picking one row filters to that label and closes the panel; picking "All messages" or re-picking the active label clears it. This matches today's boolean `pinnedOnly`, the card-chip click and the Settings row click (each sets one `label:<slug>`), and §8.1's "one label at a time (multi-select deferred)". AND/OR multi-select is explicitly deferred: AND stays expressible by typing `label:a label:b`, and a future multi-select should be OR (union) within the drop-down, never a second AND hidden in the panel.
- Whole-threads: the drop-down sets the same filter the search text sets, so the Entire threads chip (`#search-entire-threads` in `vibe-talk/web/voice.html`, `entireThreadsOn` in `vibe-talk/web/src/view/channel/searchrows.ts`, shipped with #1825 and referred to by the owner as the Whole-threads toggle #1852) composes unchanged: buckets (#1797) choose which scopes contribute hits, Entire threads chooses whether a hit brings its whole thread, and the label filter ANDs with both.
- Pinned mapping (no data migration): the `pins` tables, the snapshot store and the `pinned-log` list stay exactly as they are. "Pinned" in the drop-down is a virtual built-in label with reserved slug `pinned`: selecting it drives the existing `setPinnedOnly` path, and a new `label:pinned` search operator is special-cased on server and page to mean "is pinned in this channel" (checked against the pin set, never against derived hashtag labels). The slug `pinned` is reserved in the hashtag extractor so a typed `#pinned` cannot create a second, divergent "pinned" label; that one-tag behaviour change is called out in the note for coordb to confirm. No rows move, no schema changes, rollback stays a binary swap.
- Counts: the Pinned row badge is the current channel's pin count (the held pin list size, capped at 100) and reads "N in this channel"; hashtag and group badges are the `GET /api/v1/labels` counts over the loaded history and read "N in loaded history", group counts summed over spellings. The two denominators are different today and the panel says so rather than implying one; no API change in v1.
- Settings > Labels (#1825): one source of truth. The drop-down reads the same `GET /api/v1/labels` response (labels + groups + revision) the Settings browser reads, shows groups collapsed exactly as Settings does, and selecting a group sets `label:<display>` which the existing group expansion turns into an OR of its spellings. Renames, combines and dissolves in Settings show on the next drop-down open (refresh on open); no second store, no cached copy kept across opens.
- Keyboard and screen reader: the button carries `aria-haspopup="listbox"` and `aria-expanded`; the panel is a `listbox` labelled "Filter by label" with `option` rows carrying `aria-selected`. Focus goes to the filter box on open when the list is long and into the list otherwise; ArrowUp/ArrowDown move, Home/End jump, Enter/Space selects and returns focus to the button, Escape closes and returns focus, Tab closes. Each row announces name, count with its scope ("Pinned, 12 in this channel"), and selected state; the closed button announces the active filter ("Labels: Pinned"). Behaviour follows the existing action-menu opener discipline (`vibe-talk/web/src/view/shell/actionmenu.ts`: `aria-expanded` and `hidden` set together).
- Mobile: on phone (412x915) the panel hangs full-width under the search bar, rows at the 44px touch target the bar already uses, list capped at half the viewport height and scrollable with the filter box sticky at its head; it overlays the list rather than pushing it, so opening it with the Discuss half-pane up or the on-screen keyboard open never reflows the messages underneath.
- Build in steps (each step reviewable alone): (1) reserve `pinned` + `label:pinned` in the search language with conformance tests; (2) drop-down control and panel fed by `GET /api/v1/labels` plus the pin count, single-select wired to the existing filter paths; (3) keyboard/screen-reader pass and mobile layout; (4) dynamic end-to-end and screenshot matrix.

# Acceptance Criteria

- The search bar's pinned-threads button is a Labels drop-down; Pinned is the built-in first label option, followed by hashtag labels and groups most-used first, with a filter box that narrows long lists.
- Picking a label filters to it (single-select; AND remains available as typed `label:a label:b`); picking "All messages" or the active label again clears the filter. The Whole-threads (Entire threads) toggle and the #1797 bucket chips compose with the drop-down filter exactly as they compose with a typed `label:<slug>` search.
- Pinned still shows the channel's pinned messages from the existing snapshot store with no data migration; `label:pinned` typed in search means the same set. No other label's storage or counts change.
- Count badges: Pinned shows its in-this-channel count; hashtag labels and groups show their in-loaded-history counts, groups summed over spellings, and the panel words the two scopes so they are not confused.
- A group selected in the drop-down filters to every spelling in the group; edits made in Settings > Labels appear on the next drop-down open.
- Keyboard and screen-reader: the drop-down opens, navigates (arrows/Home/End), selects (Enter/Space), and dismisses (Escape/Tab) from the keyboard alone, with `aria-haspopup`/`aria-expanded`/`aria-selected` kept in step and the active filter announced on the closed button.
- Dynamic proof (standing rule, owner thread 185): an end-to-end test drives the real server router with the page's exact requests (only external upstreams stubbed); the feature is exercised live on a test instance and the evidence (commands, output, screenshots) is attached. A fake-client page test alone does not pass.
- Screenshot matrix (standing acceptance, owner thread 124): light and dark × keyboard open (real `visualViewport` shrink) and closed × phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the drop-down open state before/after; every image looked at, folder path and one line per image in the report.
- Page tests at phone and desktop sizes for the drop-down, in light and dark, with keyboard-open layout covered; condition-based waits only.
- `make validate` green on the final tree (run bare, never piped), then the owner-priority gate and landing as instructed.

# Notes

Prior work read before filing: #1825 vibe-talk-hashtag-labels (extractor, `Message.labels`, `label:` search, `GET /api/v1/labels`, Settings > Labels groups, Entire threads chip), #1835 label-dialog-placement and #1836 label-inline-fallback (both closed follow-ups), and `ai_docs/plans/2026-11-07-voice-overview/README.md` §8.1, which recommended Pinned as the built-in seed label in one namespace and deferred multi-select filtering. The liaison audit on #1825 recorded "Built-in labels like Pinned: not done" — this issue is that deferred step, now directed by the owner in thread 190.

Open for coordb in the design note (recommendations stated, approval asked): reserving the slug `pinned` against the hashtag extractor; single-select v1 with OR multi-select deferred; keeping the two count denominators worded rather than unified in the API.

No code until coordb approves the design note.
