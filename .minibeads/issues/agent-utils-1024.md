---
title: 'herdr-server-thread-growth: the herdr server accumulates tokio worker threads over days'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-07T21:50:00.000000000+00:00
updated_at: 2026-11-08T04:00:00.000000000+00:00
---

# Description

The long-running `herdr server` gains threads steadily:

- One host: 492 threads after 27 h (RSS 277 MB). On 2026-11-05 the previous server there had 525 threads and 23 GB RSS after 23 days, with CPU spikes of about 72 cores.
- Another host, 2026-11-07: `herdr-0.8.0 server` with 493 threads after 5 days 5 h (RSS 412 MB). By thread name: 413 `tokio-rt-worker`, 73 `herdr-pty` (exactly one per live pane: 73 panes across 7 workspaces), 3 `herdr-api-conn`, and 4 others.

So the pty threads track live panes, but `tokio-rt-worker` threads keep growing. A tokio blocking pool caps at 512 by default and lets idle threads exit after a keep-alive, so hundreds that persist are most likely blocking calls that never return: per-connection or per-pane readers, or `herdr pane run` / herdr-run tabs whose blocking reads are never joined or cancelled.

# Design

- Sample the thread count and names over an hour on a busy host while herdr-run tabs and agentctl sends come and go. Take stacks of the `tokio-rt-worker` threads (gdb `thread apply all bt`, or tokio-console if available) and find which call they block in.
- Tie the growth to an event (an API connection, a pane run, a tab close) with a reproduction loop.
- Fix it in herdr: cancel or join the blocking work when its connection or pane goes away, or move it off the blocking pool. The owner's rule is that herdr fixes go as a pull request on rrnewton/herdr; this issue tracks it from agent-utils.

# Acceptance Criteria

- A diagnosis naming the call the accumulating threads block in, and the event that leaks one, reported before the fix.
- A reproduction loop that grows the thread count on the current release and not with the fix.
- The herdr pull request link recorded here, and the thread count after a day on a fixed server.

# Notes

Filed by the devbig030 coordinator at the lead's request. Thread names from /proc/<pid>/task/*/comm.

## 2026-11-07 progress (cl-start)

- Diagnosis: there is no leak. The server holds one tokio worker per CPU (316 here), because the runtime builder sets no worker count, plus two threads per live pane: a `herdr-pty` reader and a blocking-pool thread parked in `wait4`, running the pane's `spawn_blocking(child.wait())`. The count stayed flat over an hourly sample (493-496 total, pane threads tracking 73-75 live panes).
- Two PRs on the fork, against `upstream-main` (upstream master ee5bfb50), opened by the lead as rrnewton. Neither is merged or deployed; the owner picks the restart time.
  - https://github.com/rrnewton/herdr/pull/3 (a) fix(server): bound the tokio runtime's worker threads. Threads at 0/10/40 panes: 319/339/399 on the deployed 0.9.5, 11/31/92 with the PR.
  - https://github.com/rrnewton/herdr/pull/4 (b) fix(pane): reap pane children on one thread instead of one blocking thread each. It uses one pidfd+epoll reaper thread and no SIGCHLD handler. Threads: 319/330/360 with 0 wait4 threads.
- PR descriptions: hosts/devbig030/handoffs/2026-11-07-herdr-pr-{a,b}.md in the harness repo.
- RSS growth is tracked separately in #1028 herdr-server-rss-growth.

