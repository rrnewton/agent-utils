---
title: 'agentctl-paste-tail-flush: flush unsent paste tails at delivery, in bursts, and in idle sweeps'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-01T19:39:00.000000000+00:00
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
