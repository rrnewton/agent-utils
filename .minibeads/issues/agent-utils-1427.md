---
title: 'agentctl-misroute-false-positive: delivered long briefs quarantined as probable misroutes'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-14T16:05:39+00:00
updated_at: 2026-11-14T16:05:39+00:00
---

# Description

[watermelon-full-preview] Reported by another team sharing this host: agentctl's misroute quarantine produced two false positives in about an hour. Both were multi-KB briefs (54 and 47 lines), both were actually delivered, and both seats acted on them, yet both queue records were quarantined as `PROBABLE MISROUTE` with `possibly_submitted` set. Redacted copies of the two queue records are the source evidence; the brief bodies and pane text are not reproduced here.

- Case A: the delivery error says the target shows the prompt, with many peer panes `uncertain` (the rest `absent`, none newly showing it), yet the prompt was quarantined as unattributable. A positive sighting in the target lost to `uncertain` peers, where `uncertain` only means a peer already showed matching text before the send and an old occurrence may have scrolled or redrawn.
- Case B: the delivery error says the target does not show the prompt, with peers `absent` or `uncertain`, yet the full brief was later found in the target seat's scrollback and the seat worked on it. The read-back decided before the paste had rendered, and/or its text fingerprint failed to recognise the long brief in the target: both briefs share the same sender-header opening and the same closing boilerplate, so a short head/tail fingerprint cannot tell one brief from another brief by the same sender, and a target already holding an earlier such brief shows no new count for the same tail.

The harm is the sender guidance that follows a quarantine: the sender is told the message will not be retried and to send it again. A blind resend duplicates a request that was in fact delivered and acted on.

Relevant code at filing: the attribution verdict in `read_back` (`rs/agentctl/src/subagents.rs`, `py/agentctl/subagents.py`), which quarantines whenever any peer is not `absent` unless the prompt is proven where typed input lands in the target; the read-back fingerprint (`readback_head`/`readback_tail`, first/last 40 compared characters); the fixed read-back wait (`READBACK`, 5 s, polled to a wall-clock deadline); and the quarantine notices in `rs/agentctl/src/agent_watch.rs` (`report_quarantines`) and `rs/agentctl/src/chat_service/owner_alarm.rs`, which tell the sender it will not be sent again and to send it again.

# Design

- Attribution precedence: a positive "target shows it" wins over `uncertain` on other panes. Only a peer newly showing the prompt (`fresh`), not a peer that merely held matching text before the send, can outweigh or rival positive target evidence. Existing protections stay: words appearing only in a target's tool output are not delivery evidence, and a prompt newly in exactly one other pane and not in the target is still a misroute.
- Robust fingerprint for long pasted prompts: match on a fingerprint that distinguishes one long brief from another brief by the same sender (head plus tail taken so shared sender headers and shared closing boilerplate cannot collide, or a queue-id marker where one is injected into the delivered text), never on a full-text match against the rendered screen, which wrapping, collapsing to a paste placeholder, and redraws defeat.
- No decision before the paste has rendered: wait on a condition (the fingerprint or paste placeholder appearing, the screen settling) rather than expiring a fixed delay or wall-clock budget. The logic and its tests must be robust on a hot machine (over 75% CPU), where rendering can lag far behind submission.
- Honest sender guidance under uncertainty: when a quarantine is uncertain (the prompt may have reached the target), the sender's message says "may have been delivered: read the target pane before resending", not "will not be retried, send again". Guidance that invites an immediate blind resend is reserved for cases proven not delivered.
- Regression fixtures: the two reported queue records become durable regression fixtures with brief bodies and pane text redacted down to what the test needs. No other team's agent, user, or host names, and no internal paths, may appear in the public tree.
- Both editions stay in step: the Rust agentctl (`rs/`) and any Python edition code the change touches implement the same verdict, fingerprint, waiting, and wording rules, and the cross/differential tests for this path cover the change.

# Acceptance Criteria

- A regression test from case A: the target newly shows a long brief while peers are only `uncertain`/`absent`; the send is attributed to the target, not quarantined, and nothing invites a resend.
- A regression test from case B: a long brief whose fingerprint distinguishes it from an earlier same-sender, same-boilerplate brief already in the target is recognised in the target even when the shared tail count does not increase; the send is attributed, not quarantined.
- A test that a decision waits for the paste to render: a target that shows the fingerprint only after a delay exceeding any fixed short budget is still attributed, with no fixed-delay or wall-clock-budget expiry deciding the verdict, and the test passes under CPU load.
- A test of the uncertain-quarantine sender message: it says the message may have been delivered and to read the target pane before resending, and does not say "will not be retried" or instruct an immediate resend; a proven-not-delivered quarantine keeps its existing guidance.
- True-misroute behaviour is unchanged: a prompt newly in exactly one other pane and not in the target is still quarantined (and countermanded only under the existing opt-in), and a target showing the words only in tool output still does not outweigh a peer.
- The two fixtures are redacted to the minimum the tests need, `scripts/check_client_names.py` passes run bare, strict mypy passes on every changed Python file, `cargo test` passes for the touched Rust crates, and the cross/differential tests for this path pass.

# Notes

Numbered from devbig030's interim block (1400-1469). Source records were supplied read-only by the reporting team as copies; the originals and that team's registry were not read. Two cases in about an hour, both multi-KB briefs actually delivered and acted on, indicates a systematic fingerprint/timing flaw for long pastes rather than operator error. Related prior work: #704 misroute-shared-text (typed-in-target evidence outweighs peers), #953 (receipt-proved and collapsed-paste delivery), #1009 agentctl-collapsed-paste-taken.
