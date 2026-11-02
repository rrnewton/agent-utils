---
title: 'chat-stream-child-resources: the bridge''s events-stream child had 745 threads and 886 MB RSS minutes after start'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T18:18:28.748666834+00:00
updated_at: 2026-11-02T18:18:28.748666834+00:00
---

# Description

[opus 5.5] 2026-11-02 on one host: a chat bridge's provider events-stream child process had 745 threads and 886 MB RSS 5.5 min after the bridge started. A sampling trend over an hour will be attached. Find whether it is a thread-per-connection or per-subscription leak, an unbounded buffer, or expected; cap threads and memory, and surface the child's resource use in chat health.

# Notes

[opus 5.5] Trend on that bridge (one Slack tail), 7 samples over 6-66 min after start: threads 745, 746, 749, 750, 751, 749, 749; RSS 886, 848, 854, 926, 835, 819, 814 MB. Flat: no leak within the hour, but a large fixed footprint (~750 threads, ~0.85 GB). The question is whether that is expected for the events-stream child (a thread pool sized by CPU count on a 316-thread host would explain the thread count); if not, cap the pool and buffers. Also check our own bridge here for comparison.

[opus 5.5] Same here (2026-11-02, the other host, 316 CPUs): two bridges' meta-events-stream children each have exactly 745 threads and ~1.0 GB RSS. The identical thread count across hosts and bridges points to a fixed pool sized by CPU count in the vendor events-stream CLI (a XAR), not a leak in agentctl. Options: pass a worker-count option or env var if the CLI has one, run the child under a cpuset/cgroup with few CPUs so its runtime sizes the pool small, or share one stream child across bridges on a host.
