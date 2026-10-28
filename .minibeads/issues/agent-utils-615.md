---
title: 'chat-file-messages: owner messages with screenshots never reach the agent again'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-10-28T07:41:23.390896728+00:00
updated_at: 2026-10-28T07:41:23.390896728+00:00
---

# Description

[muse watermelon] Regression of #485 chat-file-messages. Owner 2026-10-28: two top-level messages with one screenshot each (ts 1793170783.059239 file F0CCB1BSZGX; ts 1793172671.826209 file F0CCMBA2069 in the bridge channel) never became bridge requests; the next text-only message arrived as request 313 within a second.

Root cause (evidence): the event stream never carries file messages, so the #485 rescan is the only path. The plugin diagnostics log shows 107 Slack rate-limit (429, conversations.history/replies, retry_after 1-2s) failures: rescan passes burst up to 21 listing calls every 90s with no retry-after handling, any single failed listing aborts the pass, and failures back off exponentially to 1800s. The two missed messages fell between failed passes (failures at 1793170690 and 1793172491, each scheduling the next pass 1800s later). The CLI transport also only parses the client error document from stdout; the live 429 document arrives on stderr, so it is misclassified as generic client_failed.

Wanted: rate-limited listings are retried in-pass after retry_after (bounded), a rate-limited pass reschedules from retry_after instead of exponential backoff to 30min, one thread listing failure does not abort the whole pass, the stderr error document is recognised, and a regression test with the fake provider covers a files-only and a text+file message delivered despite a rate-limited first listing. Recovery: both missed messages are inside the 24h rescan window, so a successful pass after deploy delivers them; no manual replay needed if deployed within the window.

# Acceptance Criteria

A files-only message and a text+file message are delivered by the rescan with file ids and download commands even when listing is first rate-limited; rate-limit backoff follows retry_after; existing rescan tests stay green.
