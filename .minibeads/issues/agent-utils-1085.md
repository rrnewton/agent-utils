---
title: 'muse-send-staged-unsubmitted: agentctl send leaves the prompt pasted in the Muse editor without Enter'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-08T19:35:00.000000000+00:00
updated_at: 2026-11-08T20:30:00.000000000+00:00
---

# Description

`agentctl send` to Muse lanes repeatedly leaves the message pasted in the Muse editor without pressing Enter, so the lane sits idle with its dispatch unread. `agentctl status` then reports `agent_status: "staged"`. The lane record carries this error:

> pane wJ:pRG agent-prompt outcome is unknown; prompt may have been submitted: ... the pasted prompt was not observed as the whole Muse editor within 60s; Enter was not sent; taking the paste back out of the editor failed (the Muse composer did not change after 3 clearing key(s); it still holds text), so it may be left there

Another harness's coordinator on devbig030 saw this at least 8 times between 18:50Z and 19:22Z on 2026-11-08, across seven Muse lanes (watermelon-preview, Herdr panes wJ:pR*). Each was cleared by hand with `herdr pane send-keys <pane> Enter`. Priority-1 dispatches sat unread for up to an hour. devbig014 may have seen it too: a Muse lane there ignored a delivered message until it was resent.

The sender is never told: it sees `pending` or "outcome unknown", not "not delivered".

# Design

1. After a staged outcome, either retry the submission safely (re-observe the editor; send Enter only when the editor holds exactly this prompt and nothing else; never double-submit) or report `outcome: staged` to the sender as NOT delivered.
2. Expose `staged` (draft left in the editor by agentctl) in status in a way stuck scans and `watch-agents` find; the watcher alerts on it.
3. Root cause: suspects are the 60 s paste-observation window on long messages under host load, and how Muse's editor renders a large paste (wrapping, truncation, a collapsed paste placeholder) so the whole-editor match never succeeds. Measure the time from paste to a matching editor for long messages under load, and fix the cause rather than only lengthening the window.

# Acceptance Criteria

- A long message (several KB, many lines) sent to a real Muse pane under gate load is submitted exactly once, or the sender gets an explicit not-delivered outcome; never a silent staged draft.
- `agentctl status` and `watch-agents` surface a staged draft left by agentctl; a test covers the alert.
- Regression tests cover the observation and recovery path (match, partial match, clear failure).
- The measured root cause is recorded under Notes.

# Notes

Filed by the devbig030 coordinator at the lead's request. Pane captures from the reporting harness are available through the lead.

## Resolution

Measured on 2026-11-08 (ET times in parentheses) with a throwaway Muse 1.4.15 pane (watermelon-full-preview, xhigh), read-only captures of seven long-lived Muse panes in another harness on the same host (redacted here as consumer-a lanes), and one long-lived lane of this harness. The host's load average was 110-135 throughout.

**What it is not.** A fresh Muse pane took every paste at once, even with heavy host load: 184 B, 2.6 KB, 5.8 KB and 11 KB briefs, idle or mid-turn (a foreground command, a long thinking step, a sub-agent), were drawn within one 0.3 s poll and delivered. Long pastes collapse to `[Pasted Content N chars]` above about 1,050 characters, and multi-line literal pastes of up to 16 rows drew whole. The failing deliveries were mostly short: 300-560 characters on two lines, a sender header and one paragraph. Neither paste size nor the 60 s window is the cause.

**Cause 1: Muse stops reading its terminal for minutes, and agentctl's clearing keys then corrupt the editor.** All 55 failures behind the report, and three more in this harness's own lane, were follow-ups typed into a running turn (`muse_followup`). Each followed the same sequence: the editor was empty at the baseline, the paste never showed as the whole editor within 60 s, and the withdraw sent ctrl+e, ctrl+u and backspace, with 100 ms waits, and saw no change (33 cases). In 20 more cases the sender's own process was terminated at exactly that moment (its `timeout 60` wrapper) and the first clearing key was refused with "control command was cancelled". Two read-only captures show the result:
- A consumer-a lane, still "Calling tools" at 10m39s, had in its editor the sender header of delivery A (typed 19:27:15Z) immediately followed by the whole of delivery B (typed 19:28:18Z). A's body was gone.
- This harness's lane (Muse 1.4.15, mid "Thinking") had a three-line prompt typed at 19:52:33Z (14:52 ET). At 19:53:33Z it had still not drawn it whole. By 19:55:34Z the editor showed only lines 1-2, and still did at 20:01:46Z, after the turn had ended at about 19:57Z. The identical 320-character prompt pasted into the fresh probe drew all three rows within 1.5 s, and the matcher accepted it as exact.

Both leftovers are exactly what a pane that takes in its input more than 60 s late produces: the paste, then ctrl+e, then ctrl+u (deletes the last line), then backspace (deletes the newline before it), and then, in the first case, the next delivery's paste appended. So the delivered draft was cut down by agentctl's own late keys, two prompts were glued together, message bodies were lost, and a person pressing Enter by hand submitted the mangled text. Herdr reads stayed fast (the clears took about 0.5 s). The panes kept rendering (their timers advanced) but did not consume input. The affected Muse processes had all been running for hours, and their binary had been replaced on disk since they started (`/proc/PID/exe ... (deleted)`). Their sessions were 445-646 MB, against 3.5 MB for the probe. The other harness is restarting its lanes on the current binary to test whether the input stall recurs on fresh sessions (A/B pending).

**Cause 2: tabs.** Every long-paste failure (14, all to a single coordinator lane) held a tab. Measured on the probe, Muse's placeholder count is code points with CRLF as one, as agentctl assumed, except that each tab counts 4 wherever it sits, and FF, VT, BEL and DEL count 0. The count never matched, so each paste timed out. Reproduced end to end with `agentctl send` (a 2,690-character brief with one tab showed as 2,693).

**Fix.**
- `muse_paste_length` (Rust, and Python `_muse_paste_length`) counts tabs as 4 and drops other C0 controls and DEL. Stale-draft vouching accepts either count.
- When the stage window ends with the editor still empty, or showing only the start of the paste (`muse_verified_process_paste_drawn`: Nothing / Part), no key is sent. The editor is left untouched, and the delivery returns NotStaged `stranded_prompt: input_stalled: late_paste: ... NOT delivered yet`, so the prompt stays pending (exit 75) and is never typed again. A cancelled delivery does the same. The idle path no longer gives up after 2 s on such a paste; it waits out the rest of the 60 s first. An editor that shows something else is withdrawn as before.
- A later pass (`muse_stranded_retry`) presses Enter only when the editor shows the whole paste, so it is submitted once. It counts the prompt delivered if it appears as a user turn or queued input. After the 300 s late-paste window it reports the paste lost, still never retyping it. A stranded pending record now vouches for its own late-drawn draft, so a readiness wait submits it instead of refusing it as foreign.
- While a prompt is input-stalled, every other prompt to that queue, owner and urgent ones included, waits untyped, with `input_stalled: pane input stalled: queued behind message ID`. The stranded prompt is tried first in each drain.
- `agentctl status` lists such prompts under `input_stalled`. watch-agents' held-queue escalation reports kind `input-stalled` once per hold, with a hint not to type into the pane and to restart the harness on the same session if it stays frozen. The owner alarm uses the same kind.

Tests: placeholder counts (tabs, controls, CRLF), classification of nothing / partial / whole / glued / foreign / placeholder drafts, and a several-KB brief drawn in part then whole. Delivery: an undrawn or half-drawn paste, idle and busy, is left untouched and reported not delivered. A later owner send and an urgent send queue untyped. After release, each prompt is submitted exactly once with one Enter each. A paste that never draws is reported lost and releases the queue. Also: stranded-record vouching, the watch-agents `input-stalled` alert, and Python parity of the count.

Not verified: why Muse stops reading input. That is in Muse, and the restart A/B will show whether session age or the replaced binary matters.
