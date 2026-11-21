---
title: 'vibe-talk-documents-newest-chip: the channel Newest chip shows over the Documents reader'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- documents
- channel-view
- ui
created_at: 2026-11-21T00:22:47-05:00
updated_at: 2026-11-21T00:22:47-05:00
---

# Description

[watermelon-full-preview] A "↓ Newest" chip (the channel list's jump-to-newest scroll tool) stands over the Documents reader in screenshots, on phone and desktop, in every scheme: it belongs to the channel list behind the reader, and the reader shares its scroller, so the chip has no message to jump to there. Found while shooting #2006 vibe-talk-documents-react (owner shot review, 2026-11-21). Evidence: the 64dedcc3 shot folders for #2006 (documents-dark-phone412-after, documents-light-desktop1280-after, and the React/notice frames) all show the enabled Newest pill over the open document. Pre-existing, not from #2006 and not from #2003/#2004: the same chip stands over the document in the earlier 64dedcc3-predecessor shots for the read start rule (documents_read_browser at 707b7802), and the chip's owner is the channel scroll tools (`jump-newest`), untouched by those changes; the Documents reader reusing `#scroll-area` is what carries it across views.

# Design

Hide the scroll-tools chips while the Documents reader (and, by the same reasoning, any non-channel view) is up, or scope the Newest chip's render to the channel view being shown. The chip must stand again exactly as it was when the reader closes — no state of the channel list's unseen marking may be consumed by having hidden it.

# Acceptance Criteria

- No "↓ Newest" chip stands over the Documents reader (phone and desktop, light and dark), while it still works in the channel views.
- Screenshot proof in both states.

# Notes

[watermelon-full-preview] mu-readstart, filed from the #2006 shot review at the coordinator's direction ("If main does the same, file it and don't fix it here"). It does: the chip predates #2006 (see Description). Unassigned.
