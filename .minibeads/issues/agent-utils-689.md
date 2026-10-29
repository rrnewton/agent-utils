---
title: 'muse-bridge-readback-tail: read-back attributes a prompt by a tail every bridge prompt shares'
status: open
priority: 1
issue_type: bug
created_at: 2026-10-29T15:54:23.493095582+00:00
updated_at: 2026-10-29T15:54:23.493095582+00:00
---

# Description

[opus 5.5] Found by #679 muse-bridge-study. A send's read-back (`read_back` /
`input_occurrences` in rs/agentctl/src/subagents.rs) looks for a prompt in the target and in
every other registered pane by its last 40 characters (`readback_tail`). Every chat-bridge
request prompt ends with the same boilerplate ("... print that reply between the two lines at
the end of your turn instead."), so any other pane that ever received a bridge prompt shows an
old occurrence ("uncertain") or a new one from its own bridge ("fresh"), the send is ambiguous,
and the request is quarantined as `PROBABLE MISROUTE` and recorded delivery_uncertain.

Reproduced on a private test bridge: a Claude Code subject got 5/5 requests recorded
delivery_uncertain (no receipt, never retried) while it received and answered every one in
3-4 s, because four Muse subjects on other bridges showed bridge prompts. A Muse coordinator on
another host shows the same quarantine (`other panes {... "fresh"}`).

Wanted: attribute by something distinctive to this prompt (its whole normalized text, a hash,
or the request/reply ID line), not a shared tail. The bridge-robustness lane has been told;
Muse panes no longer depend on read_back since #678 muse-bridge-latency.
