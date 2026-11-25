---
title: 'vibe-talk-glossary-term-index: tapping a glossary term lists the messages that mention it; the glossary is the superset of labels'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T08:50:00-05:00
updated_at: 2026-11-25T08:50:00-05:00
---

# Description

Owner request (2026-11-25, part C, after the glossary popup of #2233 vibe-talk-glossary-popup works): "After that works maybe click a term to go to index of messages that mention it. Just like labeled and probably sharing infra. I suggest the glossary should be the superset of labels and should be the implicitly created ones. The ones that actually start with hash are the explicit list that shows when I filter on labels."

# Design

- In the full-screen glossary (#2233), tapping a term opens an index of the messages that mention it, the same way a label opens its labelled messages (#1825 hashtag labels), sharing that index and its view rather than building a second one.
- The glossary becomes the superset of labels: every label is also a glossary term, and glossary terms are the implicitly created ones. Only the explicit hash-prefixed labels show in the labels filter, as today.
- Details (how mentions are matched, how label and glossary entries merge, which route serves the index) are to be designed when this is picked up.

# Acceptance Criteria

- Tapping a glossary term lists the messages that mention it, through the same infrastructure labels use, at phone and desktop sizes.
- Every label appears in the glossary; the labels filter still lists only explicit hash labels.
- Page tests, a real-server test of the route the page calls, and the screenshot matrix.

# Notes

Not started; waits for #2233 to land.
