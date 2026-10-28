---
title: 'vibe-talk-reaction-latency: bridge reactions reach vibe-talk without a Slack round trip'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-28T17:39:22.154701197+00:00
updated_at: 2026-10-28T17:39:22.154701197+00:00
---

# Description

[opus 5.5] owner 12:09 ET: reactions (bridge eyes) appear in vibe-talk 30-60+ s after Slack shows them. Log: message 1793203612.058579 (C0C91RW5FAR) reached vibe-talk via live/tail at 16:06:52.858 UTC, but the next live/tail POST (carrying reactions) came at 16:09:39. Find whether the events tail batches/delays reaction.created, or vibe-talk only refreshes reactions on relist (180 s now); make reactions near-real-time. Owner 12:18 ET: the bridge's first reaction (admitted) must reach the page without depending on Slack: map and time every hop on today's path (vibe-talk post -> Slack -> bridge events tail -> bridge reactions.add -> Slack -> vibe-talk tail -> live/tail ingest -> page) from both logs, and design a direct bridge->vibe-talk receipt (authenticated push to the vibe-talk server, no Slack in the loop; the Slack reaction still posted for native clients). Same channel could carry the B-number metadata (#658).
