---
title: 'vibe-talk-wtf-lookup-log-and-replay: log every WTF lookup privately and replay misses to improve the engine'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T08:55:00-05:00
updated_at: 2026-11-25T08:55:00-05:00
---

# Description

Part A of an owner request: WTF? lookups are not logged anywhere the team can learn from, and the owner has not yet seen one succeed. Every lookup should leave a private, local record (the selection, where it was pressed, each query tried, what each source returned, the answer or the miss, and timings), and a replay tool should re-run logged lookups, misses first, against a local server so an engine change can be measured by its hit rate before and after. Real misses then drive engine fixes. The first known miss: a lookup of a long selection of the form "Row N, the <qualifier> gate test" in a status message found nothing after querying the whole phrase, a shorter phrase and the first word, taking over nine seconds, although "Row N" is defined in earlier status messages of the same channel. The query plan's cap is spent on long windows, so the short head noun phrase is never searched, and the channel's own recent history is never scanned. A later step runs summaries on a chosen channel, harvests their "terms I don't understand" sections (existing summaries included), drives the lookup engine with them, and proposes glossary entries.

# Design

- A private lookup log in its own subdirectory of the server's state directory (never in the repository): one JSON line per lookup, files and directory owner-only, bounded by size and age, constant work per write, no secrets or tokens.
- A replay command that reads the log and re-runs its lookups against a local loopback server, cache-only, with a retrieval-only mode that makes no model calls, and reports the hit rate per run so two runs compare.
- Engine: split a selection into its head noun phrase and its qualifier, query the head first, scan the current channel's recent cached history for it on word boundaries, and rank by definitional fit and qualifier overlap before falling back to the existing windows.
- Learned definitions stay marked as learned and unreviewed and never feed the Discuss warm-up (#2156, #2189).
- The design goes to the coordinator for review before any code; the summary-harvest loop runs only after its scope is confirmed.

# Acceptance Criteria

- Each lookup outcome (reviewed hit, learned hit, found, each kind of miss, error) appends one record; files are mode 0600 in a 0700 directory; size and age caps hold; a failed append warns and never fails the lookup.
- The replay command runs against a local server only, refuses a non-loopback address, and prints before/after hit rates; tests cover its parsing and reporting without wall-clock dependence.
- A synthetic regression through the real router (only the model stubbed) reproduces the head-noun-phrase miss shape and passes after the engine change.
- Fixtures, tests and commits hold no real channel content, names or ids.

# Notes

Filed by the lane from owner request 372 (part A); the glossary page and term index are separate parts.
