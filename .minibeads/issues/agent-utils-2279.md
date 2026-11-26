---
title: 'vibe-talk-post-facto-labels: add any label, #hashtag labels included, to a message from its menu after the fact'
status: open
priority: 0
issue_type: feature
assignee: cl-labels
labels:
- vibe-talk
- labels
- owner-ui
created_at: 2026-11-26T03:45:00-05:00
updated_at: 2026-11-26T03:45:00-05:00
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

Design note goes to coordb before any code (the liaison reviews it); this section records it once approved. Points it must settle: server-side storage with an origin (`manual` vs `hashtag`) merged into `Message.labels` at serve time, in a representation where a typed and a menu-added `#kvm` are one label (checked against #1825's slug = bare folded token, origin = hashtag); rollback safety like the label-group tables (outside the migration ladder, or reversible); no message text in logs; the `#` subset rule; menu UX on phone and desktop; removal semantics; live events.

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
