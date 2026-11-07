---
title: 'herdr-server-rss-growth: a herdr server reached 23 GB RSS after 23 days'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-07T22:05:00.000000000+00:00
updated_at: 2026-11-07T22:05:00.000000000+00:00
---

# Description

On 2026-11-05 a herdr server that had run for 23 days held 23 GB RSS (and 525 threads, with CPU spikes of about 72 cores) before it was replaced. #1024 herdr-server-thread-growth showed the threads are a host-sized runtime plus two per live pane, which explains well under 1 GB, so the memory growth is a separate question.

Current servers: 277 MB after 27 h on one host, 412 MB after 5 d 6 h on another (2026-11-07).

# Design

- Sample each host's herdr server every hour: RSS, PSS where available, live panes and tabs, and the count of herdr-run tabs opened and retired since the last sample. Use a read-only collector that never attaches to the server, writing one JSON line per sample under ~/.local/state.
- Correlate RSS with pane and tab churn (scrollback buffers kept for closed panes? per-tab state never freed? API connection buffers?) before guessing.
- Once a growth driver is found, build a reproduction loop against a test server and fix it in herdr, as a pull request on rrnewton/herdr.

# Acceptance Criteria

- At least 24 h of hourly samples from both hosts, with the correlation stated (what RSS grows with, per event).
- A reproduction that grows RSS on the current release and not with the fix.
- The herdr pull request link recorded here.

# Notes

Filed by the devbig030 coordinator at the lead's request. Related: #1024 herdr-server-thread-growth. Deploying a new herdr server restarts every pane on the host, so the owner picks the time.
