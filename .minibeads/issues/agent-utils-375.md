---
title: 'read-aloud-latency: the first tap on a message waits tens of seconds before it is heard while the page prepares every thread on screen at once'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T17:03:15.059755423+00:00
updated_at: 2026-10-19T17:03:15.059755423+00:00
---

# Description

[opus 5.5] Follow-up to #7 read-aloud-latency and #191 read-aloud-first-lag. Owner report 2026-10-19: when he first taps a message to read it there is a very significant lag before it starts. The server log at the time shows ten preparation requests for one channel in flight together, each taking 46 to 53 s and all failing, then the voice session ending on a provider error. Turning Read on, or any redraw of the list, sends one preparation per thread on screen, all at once, for every loaded row; on a chat service whose lookups queue behind one upstream slot, they wait on each other, and the tapped row's own preparation queues behind all of them. Find where the time from tap to first sound goes, cut it, and report the numbers.

# Acceptance Criteria

At most one background preparation is in flight at a time, rows on screen first; a tap's own preparation never waits behind it (the background one is cancelled and resumes afterwards). Content-free timing says how long the provider took to its first audio and for how many characters. Page tests count the preparations in flight, with a negative control that fails on the old all-at-once behaviour; a note under ai_docs/transient/ gives tap-to-first-audio and block-boundary numbers measured on a loopback instance with a fake voice backend.
