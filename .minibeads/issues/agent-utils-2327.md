---
title: 'chat-health-rescan-hold-span: chat health reports a first-to-last span of hold lines as hours of owner-rescan hold'
status: open
priority: 2
issue_type: bug
labels:
- chat
- agentctl
- slack
created_at: 2026-11-26T19:35:00-05:00
updated_at: 2026-11-26T19:35:00-05:00
---

# Description

`chat health` (rs/agentctl/src/chat_service/health.rs) said `DEGRADED: the owner rescan has been held back by the host listing budget for 6.0 h`. It takes the first and last `rescan: the host listing budget holds` lines in the last 512 KB of the plugin diagnostics log and reports the time between them, so any hold lines spread over the tail read as one continuous hold as long as the tail (about 6 h of log). The same log for 8:38 AM to 7:10 PM ET on 2026-11-26 shows 287 successful rescan passes, 159 held and 128 rate-limited. The median gap between successful passes was 121 s and the longest 337 s, and every held pass had already listed 2 to 5 channels. The plugin lists owner channels before relay channels, so owner file messages, which only the rescan delivers, still arrived within minutes. The headline sent people looking for a 6-hour file-delivery outage that did not exist, and it would hide a real one behind the same text.

# Design

- Read the successful-pass lines (`rescan: pass made ...`) as well as the hold lines in the same tail, and report the longest gap between successful passes and the held fraction over the window the tail covers, naming that window.
- Degraded only when the current gap since the last successful pass exceeds a bound (for example 15 minutes) or the longest gap in the window does; otherwise an info line with the held fraction.
- Let the plugin's hold line say whether the owner channels were already listed in that pass, so health need not infer it from the order.

# Acceptance Criteria

- Unit tests with synthetic plugin logs: holds alternating with successful passes are not degraded (info line with the held fraction); no successful pass for longer than the bound is degraded and names the gap; the window is stated.
- On the trial host's current log the headline no longer claims a multi-hour hold.

# Notes

Found in the #2108 debugging pass (owner request 392, 2026-11-26). The rate limiting behind the hold lines is #2328. Follows #1892, which added the check.
