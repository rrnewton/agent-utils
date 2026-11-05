---
title: 'chat-receipt-needs-evidence: no delivered receipt without positive delivery evidence, for any harness'
status: closed
priority: 0
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T15:54:58.746428454+00:00
updated_at: 2026-11-05T15:54:58.746428454+00:00
---

# Description

[opus 5.5] OWNER REQUEST (2026-11-05, 10:53 AM ET), P0. The owner's words, verbatim: "…Is this agentterm TUI still running under 'unknown' harness type in the bridge? That's ok but I want to fix it to be better than this. Unknown should still never justify false-positive claim of delivery to the agent when nothing is delivered. That's the behavior I'm seeing now."

On a consumer team's bridge on this host (target: an agentcloud session viewed through agentterm, so the harness is unknown), the bridge shows a "delivered" receipt (the delivery reaction) when nothing reached the agent.

# Design

1. Reproduce the false receipt on that bridge: the delivery path, the evidence it used, and why the paste returning was treated as delivery.
2. One rule for every harness, unknown included: a receipt or "delivered" state requires positive delivery evidence. That means one of:
   - the prompt seen in the agent's input area;
   - the prompt in its transcript or session history (for an agentcloud session, the session history shows the message);
   - an explicit harness acknowledgement.

   Without evidence, the request is uncertain: it is retried under the existing Enter-under-uncertainty rules and alarmed per the backoff policy. A paste returning is never evidence.
3. Then first-class agentterm support (#852 parts B and D): recognition, plus idleness for reply reminders.

# Acceptance Criteria

- Test: an unknown harness with no delivery evidence gets no receipt, and the request is uncertain, retried and alarmed per policy.
- Test: when evidence appears later (for example, the session history shows the message), the receipt follows then.
- Test: for an agentcloud session, history-based evidence is used.
- Reply path (owner reply 154 policy, folded in 2026-11-05): when the bridge follows NO transcript for a delivery (unknown or agentterm harness, headless, transcript not found), the request prompt gives ONLY "reply with `agentctl chat reply N --file FILE`, then `chat sent`", and no reply-block (fence) instructions. Fences are offered only where a transcript is followed. This is decided per delivery, including right after a `chat repoint`, and recorded in `chat status`. Test: a repoint from an agentcloud target to an unknown-harness pane switches the next prompt to command-only.
- From the owner's reply-154 spec: where a transcript IS followed (Claude, Codex or Muse with the transcript found), today's prompt stays (fences plus the reply command). Fence capture stays as a fallback, so fences seen on screen are still accepted, but they are never asked for in the no-transcript case. The prompt's mode is recorded in its delivery record as well as in `chat status`. Tests: the unknown harness gets the command-only prompt; Claude with a transcript gets the fence prompt; a transcript lost mid-session switches the mode. Docs: the agentctl-chat skill and the user guide.
- An agentterm pane attached to an agentcloud session captures replies from that session's agentcloudctl history, the same evidence source as receipts. Until then it falls under the command-only rule above.
- The consumer team verifies the fix on its bridge.

# Notes

Relayed by the lead on 2026-11-05 (owner reply 164). Assigned to cl-start, right after #891. Testing is coordinated with the consumer team through the lead.

Root cause (cl-start, 2026-11-05 ~11:15 AM ET): for an agentcloud record, `message_state` answers Processed unconditionally ("journaled at submission"). The bridge checks it before submitting, so it records the request delivered and never runs `agentcloudctl send`.

Reply path folded in at the lead's request (2026-11-05, 12:40 PM ET). During the interim repoint of the consumer team's bridge to its agentterm pane (harness unknown), inbound delivery worked, but the agent's reply blocks for requests 049 and 050 never reached Slack. The prompt still offered fences although no transcript was followed, so the agent posted both by hand with `chat publish`. #852's progress notes say prompts switch to command-only with the target kind, but that did not hold after a runtime repoint.

The reply-154 request itself (the lead, 2026-11-05 06:24 AM ET, P1) was lost in the #884 pane-lock window, in the coordinator's failed/ queue as possibly_submitted. Its full spec was recovered at 12:50 PM ET and is folded in above.

## Resolution

[opus 5.5] cl-start, 2026-11-05.

**Root cause.** For an agentcloud record, `message_state` answered `Processed` for any message ID ("journaled at submission").
- The bridge asks before it submits, so it recorded the request delivered, added the receipt and never ran `agentcloudctl send`.
- The consumer's request 2e19c566 shows this: delivered 2 ms after delivery started, with nothing in the session history.

**Receipts need evidence (agentcloud).**
- The agent's `cloud-inputs/<key>.json` (one file per input, in its own directory) is written as `attempted` before each send.
- It becomes `accepted` once `agentcloudctl send` exits 0, which it does only after journaling the input, and it keeps the seq that send printed.
- `message_state` is `Processed` only for an accepted input, or for an attempted one whose key the session history shows (`agentcloudctl history --type user_input --last 200 --raw`). That evidence is then recorded.
- Anything else is `None`: never sent, or sent with an unknown outcome and not shown in the history. The bridge then sends it again under the same idempotency key, which agentcloud journals at most once. The pending request stays under the existing retry and stall-alarm rules.

**Reply path, decided per prompt** (owner reply 154).
- A prompt asks only for `chat reply` (with no marker lines) when the bridge follows no transcript. That covers:
  - a headless agent;
  - an unknown harness, such as an agentterm pane;
  - a transcript looked for and not found, including one lost mid-session.
- Otherwise the prompt is unchanged: marker lines, plus the command where it is offered. That holds for a followed transcript or an agentcloud session's history.
- The decision is made as each prompt is composed, for the target served then. `chat repoint` now passes the unknown-harness kind, as startup does, so a repoint from a cloud target to an unknown pane switches the next prompt.
- Where caller lookup finds the bridge, the footer adds "then check it with agentctl chat sent ID". `chat help` gives the unknown-harness case its own text.
- Fence capture stays as a fallback: blocks such a target prints anyway are still captured, but they are never asked for.
- Each request record keeps its latest prompt's `reply_mode` (`blocks` or `command`). `chat status` shows `reply_path` (`command_only`, `reason`, agent, time) from `reply-path.json`.

**Tests:**
- `a_cloud_input_is_processed_only_with_evidence_it_was_journaled`: an unsent input is `None` with no agentcloud call, and becomes `Processed` after a send that exits 0.
- `an_uncertain_cloud_send_is_processed_once_the_session_history_shows_it`: the history decides. A miss and a failed history read are `None`. A hit gives `Processed` and is recorded.
- `history_seq_reads_only_user_inputs_under_the_key`.
- `an_unknown_harness_gets_a_command_only_prompt_and_its_blocks_are_still_captured`.
- `a_followed_transcript_gets_the_block_prompt_and_losing_it_switches_to_the_command`.
- `a_repoint_from_an_agentcloud_session_to_an_unknown_pane_switches_the_next_prompt`.
- `bridge_paths_deliver_capture_and_name_a_cloud_coordinator` now expects `None` for an ID that was never sent.

**Not in this change:**
- **An agentterm pane on an agentcloud session capturing replies from that session's history** (and using that history as delivery evidence) moves to #852 D. Until then, such a pane gets the command-only prompt.
- **Pane receipts for an unknown harness are unchanged.** They already require the prompt to show on screen and then leave the composer.
  - The weaker cases are still taken: "taken before Enter" (new rows under the paste) and a paste never drawn whose Enter changed the screen.
  - Requiring an echo now would break the interim agentterm pane, whose side panel splits the echoed rows. That tightening comes with #852 D, which brings history-based evidence.
- **The consumer team's verification on its bridge** is pending, through the lead.

**Follow-up (seq label).** The landed change (5c78da37) saved the first number of any successful send's output as its seq. `send-message` prints a message id, and an applied `/goal` prints its echo (it journals no input), so for those the saved seq could be wrong; the `accepted` state was right either way, since both exit only after the server's durable ack. The follow-up records a seq only for `send` whose last output line is a bare number.

**Fix-forward (live check failed, 2026-11-05 14:11 ET).** After 5c78da37 was deployed and the consumer's bridge repointed to its agentcloud session, the receipt side was honest (the request stayed pending), but nothing reached the session. The `chat run` delivery (`ManagedAgents::send_identified_with_runtime`, through the service's cancellable delivery) had no agentcloud branch, unlike `send`, `send_once`, `drain`, `withdraw` and `message_state`, so it stopped at the Herdr-only check ("this operation drives Herdr-hosted harnesses only") and retried every pass. Before 5c78da37 that was hidden: the request was recorded delivered without any send. The path now journals through `cloud_send_record` under `chat-<key>`, recording `accepted` and the seq. Tests, through the service's own delivery: `a_chat_request_to_a_cloud_agent_is_journaled_and_receipted_only_after_the_send` and `a_failed_cloud_send_leaves_the_chat_request_pending_until_the_history_shows_it` (pending and counted as stalled after a failed send; delivered without a second send once the history shows the key).

**Live check passed (consumer team, through the lead, 2026-11-05).** After 9d503632 was deployed and the consumer's bridge repointed to its agentcloud session, an owner post reached the session through the bridge. The session history shows it at seq 565630 under key `chat-8ec03bac...`, its `cloud-inputs` record is `accepted`, the request was delivered after the send, and the coordinator's reply was captured and posted.

Two notes from that check:
- **The seq was not recorded** (`"seq": null`) even with c5eb3efb. The real `agentcloudctl send` prints `delivered: input durable at seq N`, not a bare number. A deduplicated resend prints `delivered: input already durable (idempotency key K)`, which has no seq. The parser now reads the ack line, and the test fixture prints that same line.
- **The reply was captured from the agentterm pane's screen, not from the cloud session's history.** Capturing replies from history stays in #852 D.
