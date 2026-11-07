---
title: 'agentctl-own-draft-submit: drain refuses a composer holding text agentctl itself typed, so every later message waits'
status: closed
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T01:20:00.000000000+00:00
updated_at: 2026-11-07T09:59:02.724828000+00:00
closed_at: 2026-11-07T09:59:02.724828000+00:00
---

# Description

A message to a Claude lane that is mid-turn can end up as unsubmitted composer text that starts with agentctl's own recovery prefix ("may not have reached you:" or "may have arrived:") followed by the queued message. When the lane goes idle, drain refuses: "composer already holds unsubmitted text ...; refusing to append to it; nothing was typed". Every later message to that lane then stays pending until someone presses Enter in the pane.

Seen 3 times in about 20 minutes on one host on 2026-11-06, in two Claude lanes of another harness (reported by its coordinator). It has the same root as #953 coordinator-pane delivery failures: delivery into a busy Claude pane, where the prompt is queued or collapsed and not yet drawn.

# Design

- Before refusing, drain checks whether the composer text is a draft agentctl itself typed: one of its recovery prefixes plus the text of the queued or in-flight message it belongs to (match the message, not just the prefix).
- If it is, submit it with one bare Enter. Never retype it, so it cannot arrive twice. Record the outcome on that message, then continue the queue.
- Any other draft (a foreign or human draft, or a prefix with text that matches no queued message) still refuses, as today.
- Apply the same rule wherever the composer refusal is raised (drain, the watcher's notice delivery, the chat runtime).

# Acceptance Criteria

- A test: a composer holding "may have arrived: <queued message text>" for a queued message is submitted with a single Enter; the message is marked delivered once, and the next queued message is delivered.
- A test: the same for the "may not have reached you:" prefix.
- A test: a composer holding the prefix with text of no queued message, or a human draft, is refused and nothing is typed.

# Notes

Reported 2026-11-06 by the lead coordinator, from another harness's issue tracker. Related: #953 (busy-pane delivery verdicts), 46e81a3b (collapsed paste recognised on the first attempt, never retyped).

## Resolution

The stuck drafts were failed-queue digests ("may not have reached you: ..." / "may have arrived: ...") typed into a busy Claude Code pane. The pass that typed one could not confirm it and withdrew the pending digest, deleting its record, because only a digest marked stranded was kept; the text stayed in the composer, nothing in the queue matched it any more, and every later send refused to append to it.

- A pending digest whose paste was started is kept, as a stranded one is, so the stranded path can flush it with one Enter.
- A draft no record vouches for is recognised as a digest agentctl typed when every message it names is one a digest was recorded as naming and the draft, its header aside, is the start of the digest lines those messages' own records give (in `failed/`, or the archive an acknowledgement moved them to). It is submitted with one Enter, never retyped, and the recovery is noted on the failed message's record. The Claude Code and Codex composer path, the watcher's draft sweep and the chat runtime all go through this match; the agentterm path flushes such a draft the same way before it would refuse.
- A prefix on other words, a message no digest named, a message the queue does not hold, or a person's draft still refuses, and nothing is typed.

Tests: a digest without its record is submitted with one Enter by the next drain, for both labels, and the next prompt is delivered; a digest whose message was archived is found; the refusals above.

Not covered: a digest that Claude Code collapsed to a `[Pasted text #N +M lines]` placeholder shows no words to match.

