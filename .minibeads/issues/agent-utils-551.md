---
title: 'headings-real-headings: Markdown headings show as real headings in vibe-talk on both Slack posting paths'
status: closed
priority: 2
issue_type: feature
created_at: 2026-10-26T20:16:41.097314550+00:00
updated_at: 2026-10-27T00:14:17.626210254+00:00
closed_at: 2026-10-27T00:14:17.626210114+00:00
---

# Description

[opus 5.5] Owner: "I really would like headings to be actual headings inside vibe talk." The converted Slack path posts headings as bold lines and the markdown path's header blocks come back glued into the stored text; the vibe-talk Slack shim restores them only as bold lines. Make the converted path post headings so they survive (header blocks, bold-line fallback), have the shim map header blocks back to Markdown headings, and confirm vibe-talk renders h1-h3 in the timeline, thread cards and read-aloud.
