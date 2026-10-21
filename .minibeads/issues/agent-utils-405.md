---
title: 'root-under-summary: under a summarized card the raw root message is the first child'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-21T15:45:24.760264089+00:00
updated_at: 2026-10-21T15:45:24.760264089+00:00
---

# Description

[muse watermelon] The owner, 2026-10-21: with all thread rendering, both drilling into a thread and viewing the Thread List, if what is shown is a summarized card rather than the raw first message, the raw thread contents should show that 1st/root message as though it is the first child of the summary card, so he can always see the first message he started the thread from. Today the root is an ordinary first row: under Hide read it vanishes when read. Summarized means sideCardRootText(choice) is non-empty (card text is the summariser's name and summary, not the root's words). Design in ai_docs/transient/2026-10-21-threads-unpack.md.

# Acceptance Criteria

In a thread drill-in (either column) headed by a summarized card, the root row is always present first, drawn as a bridged child of the card, exempt from Hide read and read-run collapse. In unpacked Threads blocks (threads-unpack-all) a summarized card's block likewise starts with the root child. Where the card already is the raw root message there is no duplicate row and no child styling (negative control). Page tests fail before the change; Chromium screenshots in dark and light.
