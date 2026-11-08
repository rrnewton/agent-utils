---
title: 'muse-send-staged-unsubmitted: agentctl send leaves the prompt pasted in the Muse editor without Enter'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-08T19:35:00.000000000+00:00
updated_at: 2026-11-08T19:35:00.000000000+00:00
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
