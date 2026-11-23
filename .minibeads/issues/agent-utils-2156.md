---
title: 'vibe-talk-wtf-glossary-lookup: a WTF button looks up a selected term and grows a dynamic glossary'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-23T06:50:00-05:00
updated_at: 2026-11-23T06:50:00-05:00
---

# Description

Owner request: a "WTF?" button in the text-selection bar, to the left of the Quote reply chip (the close button stays on the right). Selecting an unfamiliar term (for example a project-internal phrase in another team's status message) and pressing it adds the term to a glossary: the summarizer finds what it means in recent messages, then in a cached window of linked or fetched documents, picks the logical term from context (the shorter name or the longer phrase), and writes a dynamic glossary entry with a definition and the best source link (a message, or a document down to its subsection where possible). The reader sees a placeholder, then a small popover with the definition and the source link, or a clear failure. Glossary terms then render with a distinct link style everywhere, and clicking one reopens the popover. The glossary covers all project terminology and transient agent jargon, a much larger set than the explicit labels, so it stays separate from labels.

# Design

- Today the glossary is fixed: a committed, reviewed glossary file in the Discuss warm-up, per-channel glossaries, an unknown-terms log and the summaries' unknown-terms section; definition retrieval by tool call was deliberately left as a follow-up.
- Design first (a short plan): storage for dynamic entries and how they relate to the committed file and the per-channel glossaries; the lookup flow and its budget; strictly cache-only reads (local messages and already-fetched documents, never the chat service); how the lookup fits the summarizer backend interface (#2152); rendering of known terms; privacy (no message content in commits, shots or public issues).
- Then a fast-track first slice: the button, the lookup with placeholder and popover, the dynamic entry, and the term link that reopens the popover.

# Acceptance Criteria

- The design plan is reviewed before the build.
- First slice: on a LOCAL server with fixture or cloned data, selecting a term and pressing WTF? shows a placeholder, then the definition and a working source link; the term then renders as a glossary link that reopens it; a term with no findable definition shows a clear failure; no upstream call is made.
- Shot matrix (phone and desktop, light and dark) of the button, the placeholder, the popover and a glossary-linked term.

# Notes

Owner UI request (fast-track); filed by the coordinator.
