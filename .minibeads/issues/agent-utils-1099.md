---
title: 'claude-stranded-false-lost: a stranded paste Claude Code took is reported lost and set aside as possibly submitted'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-08T23:40:00.000000000+00:00
updated_at: 2026-11-09T00:30:00.000000000+00:00
---

# Description

A prompt that a queue drain pasted into a Claude Code pane was delivered and read, yet agentctl reported it lost and the failed digest told the agent it "may not have reached you".

Example record (devbig030, 2026-11-08, message `01794170248631862715-2106562` to a Claude Code lane): pasted at about 20:00Z, not seen drawn within the in-lock stage window, so it was stranded (`stranded_at` 20:00:43Z). Eleven stranded passes then found no evidence of it: not in the composer, no queued-message marker, no user turn of it in the visible transcript. After the 300 s late-paste window it was `late_paste_lost`, and once the lost-pass bound was reached it was quarantined `possibly_submitted` (exit 76), which is what the digest reports. The agent had in fact taken and acted on it.

So the stranded retry's delivery evidence (`submission.rs`, the `runtime.stranded_retry()` branch: composer draft, queue marker, `transcript_count`, placeholder labels, `shows_run_of`) misses a prompt that Claude Code submitted when its user turn has already scrolled out of the rows read, or is drawn in a form those checks do not match. A false "lost" sends the sender a misleading not-delivered notice, and could lead a person to resend a prompt that was already acted on.

Found while working on #1085 muse-send-staged-unsubmitted, whose Muse paths use Muse's journal as delivery evidence; the generic path has no equivalent.

# Design

Before declaring a stranded paste lost on a Claude Code pane, look for the prompt in Claude Code's session transcript (the session JSONL for the pinned session), as the Muse paths consult Muse's journal: a user message whose text is this prompt (or its paste placeholder), recorded after `stranded_at` minus the stage window, is delivery, recorded as queued or verified. Only when neither the screen nor the transcript shows it is the paste lost. Keep the screen checks as they are; the transcript is an extra positive proof, never a reason to type again.

# Acceptance Criteria

- A stranded prompt that Claude Code's session transcript shows as a user message is recorded delivered, not `late_paste_lost`, and no failed-digest notice is sent for it.
- A stranded prompt absent from both the screen and the transcript is still reported lost after the window, and never typed again.
- Tests cover both, with a fixture transcript.

# Notes

Filed by cl-staged for the devbig030 coordinator.

## Resolution

The example's transcript (read for structure only) shows what happened. The prompt was queued at 20:37:28Z, typed by the drain, and stranded at 20:40:43Z, its paste not seen within the 10 s in-lock window. Claude Code wrote `queue-operation` `enqueue` with the prompt's exact text at 20:41:45Z, about a minute after the paste. Half a second later it wrote `remove` with reason `absorbed_mid_turn`, and attached the prompt to the running turn as a `queued_command`. It never drew the prompt as a user turn, so no screen check could see it. The paste was declared lost at 20:45:22Z and quarantined at 20:46:23Z. The session had begun in another slot, so its transcript sat in a different project directory from the agent's current working directory.

Fix: on a stranded retry for a Claude Code record, before the screen checks, `claude_transcript_took` reads the pinned session's transcript. It is found by the record's conversation id, in any project directory (`harness::claude` `transcript_path`), and read at most 8 MB from its end. `transcript_took_prompt` looks for a `queue-operation` `enqueue`, a `queued_command` attachment, or a `user` entry whose text, whitespace collapsed, is the prompt, written no earlier than 60 s before `stranded_at`. A match is recorded as delivered (queued) with no key and no retyping. Otherwise the screen checks and the lost-paste window decide as before.

Tests:
- Each entry shape proves the prompt was taken. An older identical entry, a prefix, a tool result, a bare `remove`, or an assistant echo does not.
- End to end through the drain, a stranded prompt is recorded delivered and never typed again once the pinned transcript shows it.
- The same text in another session's transcript, in the same project directory, does not count, nor does an entry of this session from before the strand. Such a prompt stays stranded.
