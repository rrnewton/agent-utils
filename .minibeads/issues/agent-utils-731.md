---
title: 'agentctl-paste-tail-flush: flush unsent paste tails at delivery, in bursts, and in idle sweeps'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-01T19:39:00.000000000+00:00
updated_at: 2026-11-02T01:53:02.400490783+00:00
closed_at: 2026-11-02T01:53:02.400490783+00:00
---

# Description

On devbig030, a Muse coordinator's editor twice held an agentctl-delivered
message unsent: once the whole message, once only its LAST LINE, left behind
when a multi-line paste split during a burst of consecutive deliveries.
#720 agentctl-muse-stale-draft presses Enter on a leftover draft that carries
our sender header or a paste placeholder, but a header-less tail fragment is
not recognised as ours, and idle agents are never swept.

Wanted:
1. Burst pacing: after each delivery, wait until the editor is empty AND the
   harness shows the input accepted (turn started, or queued-input indicator)
   before typing the next; plus a small minimum gap between consecutive
   deliveries to the same pane. Bounded; report when the bound is hit.
2. Ownership of fragments: editor text that is a suffix (or contiguous
   substring, ignoring wrapping/whitespace) of a message agentctl typed into
   this pane recently (from the agent's queue records) counts as ours: press
   Enter, verify the editor clears, then deliver. Text we did not type is
   never submitted or cleared; report it instead.
3. Idle sweep in watch-agents: every pass (and the quick check), read each
   agent's editor; text of ours that has sat there for ~60 s or more gets
   Enter, with a notice to the coordinator. A flush that fails twice is
   reported as blocked (to the coordinator; to the owner via the bridge when
   the agent is a chat coordinator).
4. Tests: fake Claude Code, Codex and Muse harnesses that split a multi-line
   paste and leave a tail; show the tail flushed at the next delivery AND by
   the idle sweep, and that a burst of 5 multi-line messages arrives intact
   and in order. Show a spare Enter on an empty editor is harmless.

# Resolution

Landed on main as 15c61659fc28dee57b3bf358053f3f233805084c.

- Fragment ownership in `stale_draft`: a suffix/contiguous substring of a
  framed, recently typed prompt now counts as agentctl's draft.
- Delivery recovery and drain burst pacing flush a recognised tail before
  typing more, with a minimum gap and bounded acceptance wait.
- `watch-agents` sweeps idle editors on full passes and quick checks; a
  recognised draft older than 60 seconds is submitted and noticed to the
  coordinator, and repeated flush failure is reported blocked.
- Focused fake-harness tests cover Claude Code, Codex, Muse, idle sweep,
  foreign drafts, blocked flushes, and a five-message burst.
