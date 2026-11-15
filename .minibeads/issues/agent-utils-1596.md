---
title: 'readback-composer-raw-clients: staged composer text is not delivery evidence, and raw queue clients get the read-back'
status: open
priority: 1
issue_type: bug
external_ref: gh-231
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

Two read-back gaps remain from the misroute-detection review (Gap 3, repeated prompts quarantined, is fixed by #704 misroute-shared-text and is out of scope here). Gap 1: seen_in_target counts occurrences in the full scrollback, which includes the harness composer, so a pasted-but-unsubmitted prompt (for example after a dropped Enter) can count as delivered on the paths that rely on the read-back rather than a printed receipt. Gap 2: the read-back lives in the managed workspace client; plain-client callers (the queue CLI for already-running agents, the chat bridge runtime, the foreign-agent library) get no recipient pin and no peer read-back, so a pane swapped between check and write can still report delivered.

# Design

Gap 1: before counting, strip the composer region using the same per-harness composer recognisers submission already uses (composer_view), or count only text above the composer; if the composer cannot be recognised, the observation does not count as fresh. Preserve the #704 rule for submitted text showing where typed input lands once a receipt proves submission. Gap 2: pick one of the audited options and implement it in both editions: pin what is observable at enqueue time in the queued message and verify it around the write; or run the peer read-back against registered agents even for unregistered targets; or document no detection for unregistered targets and refuse plain-client prompts to panes a registered agent owns.

# Acceptance Criteria

- Gap 1 test: the target shows the prompt only in its composer after the send; the message is not reported delivered (quarantined as unproven, or kept pending if no Enter was sent). Both editions.
- Gap 2 test: a plain-client drain whose target pane is swapped between check and write does not report delivered. Both editions.
- Existing read-back, misroute, and collapsed-paste tests keep passing, including the #704 shared-text deliveries.

# Notes

[watermelon-full-preview] Filed from GitHub #231 readback-composer-raw-clients (https://github.com/rrnewton/agent-utils/issues/231) during Request 098 triage. Related open #689 muse-bridge-readback-tail is a different defect (shared-tail attribution) and stays separate. The opt-in countermand hardening (stage 2 in the GitHub report) remains out of scope.
