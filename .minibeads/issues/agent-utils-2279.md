---
title: 'vibe-talk-post-facto-labels: add any label, #hashtag labels included, to a message from its menu after the fact'
status: closed
priority: 0
issue_type: feature
assignee: cl-labels
labels:
- vibe-talk
- labels
- owner-ui
created_at: 2026-11-26T03:45:00-05:00
updated_at: 2026-11-26T07:40:00-05:00
---

# Description

Owner requests 380 and 382 (2026-11-26 3:32 and 3:36 AM ET, owner-UI fast-track, "asap"), verbatim:

- 380: "Also In addition to pinning a message post facto of course I also want to put a label on it post facto even if I did not put a hash tag in when it was written, or the agent did not. Please get that live asap"
- 382: "Not only the same as hash tag labels but I should be able to use hash tag labels there as well to add them post facto. They should just be a subset of all labels that begin with hash symbol right ?"

Today labels are derived at serve time from typed hashtags only (#1825 vibe-talk-hashtag-labels: `Message.labels`, `label:<slug>` search, `GET /api/v1/labels`, Settings > Labels groups; #1853 vibe-talk-labels-dropdown-pinned-builtin: the Labels drop-down with Pinned as the built-in first entry). A message can be pinned after the fact from its action menu, but it cannot be labelled after the fact.

Scope confirmed with the owner by the liaison:

- ONE label set. Hashtag labels are simply the labels whose names begin with `#`. Adding `#kvm` from the menu attaches the SAME label as a typed `#kvm`: the same drop-down entry, the same `label:` search, the same card chip, the same Settings > Labels row and group. Labels without `#` are the rest (the set #2234 vibe-talk-glossary-term-index builds on; the store stays compatible with it).
- Message action menu (desktop menu and phone long-press): "Add label" picks an existing label (filterable, most-used first) or takes a new name (the extractor's token and fold rule applies to `#` names).
- Labels added from the menu are removable (any name, `#` or not). Labels derived from the text stay while the text has them, and the UI says why in a short note.
- Works everywhere hashtag labels work, updates live through a live event with no reload, and never moves the scroll position (owner rule 370).

# Design

Design note approved by coordb, reviewed by the liaison with changes (all taken):

- **One label set of names.** `#kvm` is a hashtag label, `merge queue` a plain one; the hashtag labels are exactly the names that begin with `#`. #1825's representation is kept: `Message.labels` stays the hashtag slugs (bare folded tokens, groups applied), and the menu-added `#` slugs join the typed ones there before the groups apply, so a menu-added `#kvm` IS the typed `#kvm`'s label on every surface (chips, `label:` search on server and page, `GET /api/v1/labels` counts, Settings groups and Combine, the drop-down, hashtag-only-line hiding, offline copies) with no change to those paths. New `Message.plain_labels` carries the plain names; new `Message.manual_labels` names the menu-added labels the text does not also carry: exactly the removable set.
- **Storage.** `message_labels(channel_id, message_id, name, origin, added_ms)` and `message_label_state(revision)` in the server's sqlite store, OUTSIDE the migration ladder like the label-group tables (CREATE IF NOT EXISTS at open, `user_version` unchanged, so an older binary still opens the database). Only menu labels are stored (origin `manual`); text-derived hashtags stay derived at serve time. Names stored as shown (`#kvm`, `merge queue`); `origin` leaves room for #2234's glossary. Bounds: 32 per message, 20000 in all. The storage purge erases them. An in-memory index is loaded at startup and written through.
- **Names.** One server normalizer: a leading `#` follows the extractor's token rule (ASCII letter, then `[a-z0-9_-]`, folded); anything else is a plain name (search fold, white space collapsed, 1-40 characters, no quote, colon or control character). `pinned` is reserved in both forms (#1853's built-in).
- **Search.** `label:#kvm` is the hashtag label alone; a quoted value (`label:"merge queue"`, `label:"todo"`) is the plain label alone; the bare `label:kvm` keeps matching `#kvm` (older links, hand-typed queries) and also a plain `kvm`, the one documented overlap. Every chip, drop-down row and Settings row the page makes uses an explicit form. Server, page twin and the conformance corpus change together.
- **Routes** (write scope, no MCP tool, server-local: nothing goes to the chat provider): `PUT`/`DELETE /api/v1/channels/{channel}/message-labels/{message}` with `{"name": ...}`, answering the message's labels as served now (derived from the server's cached copy, never an upstream read; a thread reply is named by its own id; ids are scoped by the channel); `GET /api/v1/channels/{channel}/message-labels`, the channel's complete list.
- **Live.** A `labels` SSE event (no id, never moves a resume cursor) on the channel stream and the all-channels stream, carrying the whole state for one message. A page folds it into its stored copy (a new object) and redraws through `followPlacement`, keeping the place (rule 370); an older revision is ignored. Missed events: every timeline and page read carries `labels_revision`; when it moves, the page reads the channel's complete list, applies every entry, strips menu labels from held messages the list no longer names, and saves the offline copy.
- **Menu.** "Add label…" after Pin message in the ⋯ menu and "Label…" in the phone's long-press sheet open one picker in the top layer (fixed, so it and its keyboard never move the list): Pinned as the built-in first row toggling the pin, the known labels most used first and filtered as you type, "Add new label …", staying open after an add; × on menu labels; a text-derived label has no × and a note says it stays while the text has it.
- **Logs.** One line per write: channel, message id, add or remove, hashtag or plain, changed, derived, ms. No label name, no message text.

# Acceptance Criteria

- From the action menu on desktop and from the phone long-press menu, "Add label" attaches an existing or new label to a message; a menu-added `#kvm` and a typed `#kvm` are one label everywhere (drop-down entry, `label:kvm` search, card chip, Settings > Labels counts and groups).
- A label without `#` can be added the same way and is filterable, searchable and shown on the card.
- Menu-added labels can be removed from the same menu; text-derived labels cannot, and the menu says why.
- The label shows on every open page through a live event with no reload, and the reading position does not move (rule 370).
- Storage is server-side and rollback-safe (an older binary still opens the database); no message text is logged.
- Page tests at phone and desktop sizes; an end-to-end test through the real server router with the page's exact requests (add a label, see it in the drop-down, search and chips, remove it); the feature exercised by hand on a local test instance with evidence.
- Screenshot matrix (light and dark, keyboard open and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in the phone shots) plus the add-label states: phone long-press menu and desktop menu, light and dark.

# Notes

Prior work to read: #1825 (extractor, derived labels, label groups outside the migration ladder), #1853 (Labels drop-down, reserved `pinned` slug), the post-facto pin path in the action menu (the pattern to follow), #2234 (glossary as the superset of labels).

## Resolution

Landed on main as 8c6b111c (owner-tier gate pass on 3e10d769, the same tree; liaison OK on the shots). An earlier gate on d09ed64e was red on offline-cache x3 (the ⋯ menu expectation lacked "Add label…", fixed together with the offline behaviour below) and documents-browser (not reproducible: 3/3 on main and 3/3 on the tree).

Beyond the design above:
- Liaison, owner rule 370: a row whose chips would push ⋯ off the header line gets them on a line of its own under the header (measureLabelRow's `data-labels-below` mark). The row a label change grows or shrinks keeps its top: this page's own write, or another device's `labels` event. It goes through the scroll owner's #2076 act path (`preservingCollapse`), which re-finds a row the redraw rebuilt by its data-id. Traces: 16/16 at 0.00 px (phone and desk; newest and mid; add, remove, remote add and remote remove).
- Offline, the picker writes nothing and says so (Pin's pattern).
- Evidence: real-router e2e `tests/message_labels.rs`; page tests `tests/js/page/labels.mjs`; screenshot scenes 150-154 (BEFORE on 3e10d769, AFTER from landed main 8c6b111c); a two-device walk on a local server.
- Follow-up filed: #2286 (a cache-serve test that waits on wall-clock time).

