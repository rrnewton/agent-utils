---
title: 'vibe-talk-screenshots-port: the screenshot step picks a free port instead of aborting on 18091'
status: open
priority: 2
issue_type: bug
assignee: cl-start
labels:
- vibe-talk
created_at: 2026-11-05T13:53:16.629288411+00:00
updated_at: 2026-11-05T13:53:16.629288411+00:00
---

# Description

The `vibe-talk.shots.screenshots` gate step hard-codes 127.0.0.1:18091 for its screenshot server. When a live process already holds that port, the step aborts the whole DAG run. On the lead host a long-running vibe-talk test stack holds 18091, so `make validate` cannot pass there at all, and at least one lane is blocked from landing.

# Design

- The step picks a free port: bind to port 0 and read it back, or try a small range. Honour an override (for example `VIBE_TALK_SCREENSHOT_PORT`).
- Pass the chosen port to the screenshot server and to every client in the step (scripts/screenshots.py and anything else that reads 18091).
- A busy port must never abort the DAG. If no port can be had, fail the step alone, with a message naming the holder if it is cheap to find.
- Search for other hard-coded fixed ports in gate steps and fix them the same way, or list them in the issue's resolution.

# Acceptance Criteria

- With a process listening on 127.0.0.1:18091, the screenshots step passes on another port, and the DAG completes.
- The override environment variable is honoured.
- No other gate step depends on a fixed port, or each remaining one is listed with a reason.

# Notes

Reported by the lead on 2026-11-05 at 08:51 AM ET (P2; it currently blocks landings validated on the lead host). Assigned to cl-start ahead of #852 C.
