---
title: 'muse-peer-approval-from-coordinator: give coordinators a supported way to answer a Muse peer-message approval dialog'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T10:10:00-05:00
updated_at: 2026-11-22T10:10:00-05:00
---

# Description

A Muse lane showed a peer-message approval dialog (allow once, reject once, allow or block for the session) for a message from another process. Enter sent through the terminal multiplexer did nothing visible, no option looked highlighted, Escape dismissed the view but left the approval pending. Nothing outside can answer it safely today.

# Design

Find Muse's supported answer path (a key sequence, a CLI or an API) and wire it into agentctl answer with the same safety rules as other approvals; find out why the worker's boxed launch failed on the relay so it was launched unboxed.

# Acceptance Criteria

- agentctl can approve or reject a pending Muse peer approval for a named lane, tested; the boxed-launch failure is explained.

# Notes

Filed by the coordinator; the owning lane refines this file.
