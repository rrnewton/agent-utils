---
title: 'cold-load-first-paint: a cold load waits ten seconds on the provider, then says Refresh failed over fresh messages'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- cache
created_at: 2026-10-28T08:07:45.838789749+00:00
updated_at: 2026-10-28T08:07:45.838789749+00:00
---

# Description

[opus 5.5] The owner, 2026-10-28 02:59 ET (screenshot 02:55, a fresh load that asked for client certificates, during the 02:56 deploy): "My load time opening the app just now was not great ... it said loading messages for a long time ... If it connected to our server quickly and everything is cached what's the problem? Should the server productively send just the last few messages chronologically so that we can show something fast? ... Also it shows me a refresh failed banner right after it did load messages from my perspective."

## Diagnosis
- The view was All on Slack, which the cache never serves at once (`serve_eligible`: Slack's Flat is not proven); a cold read of it went upstream and the cache stood in only after `timeline_fallback_ms` (10 s), with Slack rate-limited and the server restarting.
- The stand-in page carries `refresh_failed`; the page then says "Refresh failed · showing messages from 02:54" in the warn colour, over messages a minute old, and keeps saying it until the next poll, 45 s later, even while a read is under way ("Refreshing…").

## Plan
(a) A page's read on entering a view (`reason=enter`, newest) stands in from the cache after a short first-paint wait (1.5 s) instead of the full fallback; the read upstream runs on and fills the cache.
(b) After a stand-in the page reads again soon (3 s, then 10 s) instead of at the next 45 s poll; while that read is out the pill says "showing messages from HH:MM · refreshing…"; it says "Refresh failed" only when the copy is older than two minutes and no read is out; the failed state is drawn without the warn colour. A later read that lands clears it, as before.
