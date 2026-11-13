---
title: 'revive-keeps-queued-messages: a revive archives the messages queued to an agent instead of delivering them'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] On 2026-11-12 the lead coordinator on one host was revived after a 15-hour outage (#1405 restore-epoch-manager-restart). The revive archived the messages queued to it during the outage without delivering them: about 40 messages, including other coordinators' requests. The lead had to read them back from the archive by hand. A revive is exactly when queued work matters most, and the sender already got a "queued" receipt, so archiving silently breaks at-least-once delivery.

# Design

- On revive (and restore, restart, turnover relaunch), queued prompts stay queued and are delivered once the new pane is ready, oldest first, through the normal queue (priority senders first).
- A queued item may be archived only when it can't be delivered on its own terms (expired, cancelled, its target retired). Each such archive records the reason, and `agentctl status` counts archived-undelivered items so the coordinator sees them.
- A prompt typed into the old pane whose outcome is unknown stays uncertain and is reported, not duplicated and not dropped.

# Acceptance Criteria

- A fixture test: queue three messages to a dead agent, revive it, and all three are delivered in order, with none archived.
- An expired item is archived with its reason, and status shows the count.
- The restore report lists per agent how many queued items were delivered and archived (feeds #1416 team-restore-verify).

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

## Resolution

[watermelon-full-preview] Delivered in the commit that closes this issue. On revive, restore and restart (both the Rust and Python editions of agentctl), the old queue's waiting prompts are copied into the new generation's queue with their ids and `queued_at` order — the archive keeps its complete forensic copy — and drained once the new pane is ready, oldest first through the normal queue, so owner prompts still go first. Only an item that cannot be delivered on its own terms is archived undelivered: expired or tombstone-cancelled items are set aside in the new queue's `cancelled/` with their reason, an item already in flight whose outcome is unknown stays uncertain in `failed/` as possibly submitted (reported in `queued_uncertain`, never retyped), and an already-failed item stays failed. A paused agent's carried items stay pending, never archived. The revive result lists `queued_carried`, `queued_delivered`, `queued_pending`, `queued_archived` (also spelled `archived_prompts`) and `queued_uncertain`; `agentctl status` counts the archived-undelivered items as `queued_archived_undelivered`; and the restore report lists per-agent `queued_delivered`, `queued_archived`, `queued_uncertain` and `queued_pending` counts for #1416 team-restore-verify. Fixture tests with stubbed Herdr cover the three-messages-delivered-in-order case, the expired-item case with the status count, the uncertain in-flight case, and the restore-report counts. Turnover keeps the same record, token and queue, so its queued messages already survived and needed no change.

Gate record (2026-11-12): the contained gate on the delivering commit passed 65 of 68 steps. Its one divergence was in the cross-edition differential's paused revive scenario: that record is deliberately paused, so the carried prompt correctly stays pending rather than being typed into a paused pane. The differential expectation was fixed for that paused case (`queued_pending == ["pending-work"]`, the item staying in `inbox/`, and the archived list `['uncertain', 'interrupted']`), with no product-code change. In the contained rerun of `cross.agentctl.differential-2`, all eight such cases (`primary/revive/{python-to-rust,rust-to-python}/{ready,published}/(no-relaunch|archived-prompts-reported)`) ran through and the step reported 0 divergences (`grep -c DIVERGENCE /tmp/mu-revq-rerun.log` == 0, each case's new queue holding `failed/interrupted.json`, `failed/interrupted.json.error`, `failed/uncertain.json` and `inbox/pending-work.json`) before the step hit its CPU time budget in unrelated later cases, the waived class. Waived reds: the chat SIGTERM test, the #1477 restore drill (`test_restore_drill_happy_path_fresh_pane_slot_and_rerun`, test-side; its fix was gating separately), and the herdr-agent-differential CPU-TIMEOUT with no divergence.

Combined-tree record: after the gate, #1359 (restore reports out of `agents/`, `b73d2163`) landed on overlapping files, so the change was rebased onto github/main `13bcf88828` keeping both behaviours (queued-message carry and the report-directory move with recordless-`agents/` skipping), then verified contained (each under `systemd-run --user --scope -p CPUQuota=800% -p MemoryMax=16G -- nice -n 19 ionice -c 3`): Rust revive/restore/list/migrate filters (102 passed, then 75 restore passed), Python revive lifecycle (97 passed) and test_agentctl_sessions (188 passed), strict mypy clean, clippy clean, fmt check clean. The revive and differential code is byte-identical between the gated tree and the combined tree, so the eight differential paused-revive cases above stand for the combined tree; landed with `land-main --replay-base 13bcf88828 --allow-overlap`.
