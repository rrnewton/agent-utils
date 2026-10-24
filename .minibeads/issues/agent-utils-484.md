---
title: 'summary-card-threshold: no summary card before a thread''s third message'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-24T10:13:27.591543459+00:00
updated_at: 2026-10-24T11:45:45.996123342+00:00
closed_at: 2026-10-24T11:45:45.996123282+00:00
---

# Description

[muse watermelon] The owner (2026-10-24): "let's not do a summary card for a thread with one question and one response; let's wait till a 3rd message." Wanted: a thread with just a root and one reply shows its real message words on its card, no summariser name or sentence, and no model call is spent naming it. The threshold lives in one named, documented constant on the server: the naming schedule, the queue decision, the worker's model call and what a page is served all follow it. A name or sentence filed before the threshold (or for a thread that later lost messages) is kept on file but not served while the thread is known to be under the threshold; it shows again from the third message.

# Acceptance Criteria

A 2-message thread is never queued for naming when its count is known, the worker spends no model call on a thread its read finds under the threshold, and pages are served no display name or summary sentence for a thread known to be under it; from 3 messages naming proceeds on the existing backoff; unit tests pin the schedule and the queue decision.
