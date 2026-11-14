---
title: 'discuss-cite-owner-messages: allow the Discuss summary to cite the owner'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T16:24:00+00:00
updated_at: 2026-11-14T16:24:00+00:00
---

# Description

[muse watermelon] The owner's exact wording on 11-12 at 12:09 PM ET (cache record timestamp 2026-11-12T17:09:00.472369Z), allowing the Discuss summary to reference his own messages:

> Oh the cutoff sentence wasn't very important
>
> > It would be ok for the summary to sometimes move towards mentioning what I said in context "XYZ response to your message ABC" (which could link my message).
>
> I don't know if it's worth including any guidance to that effect. The important thing is focusing on what the agents have accomplished so let's not have any specific instruction about that. But we certainly could ALLOW references to my messages in the summary text.

The coordinator confirmed at 12:09 PM ET the same minute: no special instruction to mention the owner's messages, the focus stays on what the agents accomplished, but the summary is allowed to cite them when it helps, for example "done in response to your (10:42) ask", with a link like any other citation. Only the link validation changed with #1384 discuss-exclude-owner-messages; the shipped prompt in `vibe-talk/src/catchup.rs` still told the model to cite only "[UNREAD], non-[OWNER]" lines and to "never restate" [OWNER] messages, so the model never emitted the citations the validator would now keep.

# Design

- Rewrite `DEFAULT_SYSTEM_PROMPT` in `vibe-talk/src/catchup.rs` to be permissive, not encouraging: [UNREAD] stays the main news and [READ] stays background; an [OWNER] message may be referred to and cited when it carries a decision, a question, or context the briefing depends on, always named as the owner's and never presented as news from someone else; the briefing must not be padded by restating what the owner just said. No instruction asks the model to seek out owner mentions.
- Citations may be copied from any line in the window, including an [OWNER] line the bullet relies on. The "Nothing new" reply still triggers on no [UNREAD] news, so owner words alone never create a briefing.
- Link validation needs no change: the catch-up, delta, and follow-up handlers already validate against every message sent to the model, so an owner citation survives `validate_cites` and `shape_answer` renders it as an in-app (HH:MM) anchor through the trusted set. Owner messages stay out of `unread_ids`, the counts, and the footer (#1384).
- Sibling #1356 discuss-readonly-tools adds read-only tools to follow-ups on the same `effective_system_prompt`; this change touches only the prompt text and its tests and leaves the tool wiring and transcript storage to that lane.

# Acceptance Criteria

- The shipped prompt permits an owner citation, requires owner attribution, and forbids padding with owner restatements; it no longer contains the unread-only citation ban or the "non-[OWNER]" restriction. A prompt-text test pins this.
- A summary citing an [OWNER] message passes link validation (`validate_cites`), shapes to a (HH:MM) link (`shape_answer`), and renders as an in-app anchor, while a hallucinated id is still dropped and the owner message stays out of the unread set. A test pins this end to end.
- Existing catch-up behaviour is unchanged: owner messages remain context, counts and footer still exclude them, and the full `catchup::` test module passes.

# Notes

Owner request 11-12, passed on by the coordinator; #1384 discuss-exclude-owner-messages landed the validation half. Parent: #1316 catchup-side-agent. Sibling: #1356 discuss-readonly-tools (read-only tools for Discuss follow-ups, same prompt area; coordinated directly, no file overlap beyond the shared system prompt).
