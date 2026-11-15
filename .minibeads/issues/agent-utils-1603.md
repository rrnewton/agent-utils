---
title: 'agentctl-orphaned-harness: name a live harness orphaned by its exited launcher instead of stalling silently'
status: open
priority: 1
issue_type: bug
external_ref: gh-296
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

When a two-process harness launcher exits and its native child survives re-parented in a leaderless process group, the pane shell reclaims the foreground, delivery stalls for hours behind an unknown agent status, and nothing states the harness is alive but orphaned. No status/doctor finding detects "anchored pid gone, same-session harness alive in a non-foreground process group", and the held-queue escalation does not classify a queue blocked behind unknown status. Separately, a record whose top-level native_session disagrees with launch_spec.native_session can revive into the wrong conversation with no warning: effective_native_session silently prefers the top-level value when both are set, which is exactly the reported record shape (top-level wrong, launch spec correct).

# Design

Status/doctor detects "anchored harness pid gone but a same-kind, same-session process alive outside the pane foreground group" and names it with the resume/anchor remedy. Revive refuses (or warns loudly) when the two recorded session fields disagree, and never resumes a session bound to another registered agent. Watch escalation treats a queue blocked behind unknown status past the stale bound as a held queue.

# Acceptance Criteria

- A fixture orphan (launcher dead, child alive in a background group) is named by status/doctor with a remedy.
- A record with disagreeing session fields refuses revive naming both ids.
- A queue held behind unknown status past the stale bound tells the coordinator once.
- A live foreground harness is never reported orphaned.

# Notes

[watermelon-full-preview] Filed from GitHub #296 agentctl-orphaned-harness (https://github.com/rrnewton/agent-utils/issues/296) during Request 098 triage. Closed #928 native-session-collision and #948 cover collision capture and single-reader fallback only.
