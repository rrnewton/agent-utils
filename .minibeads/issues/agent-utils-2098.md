---
title: 'agentctl-chat-bridge-write-burst: find and stop the chat bridge's multi-GB/min write bursts'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T06:30:00-05:00
updated_at: 2026-11-22T06:30:00-05:00
---

# Description

A consumer measured one chat bridge (agentctl chat run) as the host's top writer: about 6 GB/min of device writes over a 20 s sample. A later 10 s sample of the same unit showed almost nothing, so it comes in bursts. The bridge state holds several large whole-file JSON stores (short refs, reply aliases, request and reply directories) that may be rewritten whole on each message or pass.

# Design

Find which files the bridge rewrites and why (write-path audit plus a write-count test); make each store write only on change and incrementally (append-only journal or per-entry files) instead of rewriting a large file per event; bound any rescan's writes.

# Acceptance Criteria

- A test drives N messages and a rescan through the bridge and asserts the bytes written grow with the change, not with the store size; the burst cause is named.

# Notes

Filed by the coordinator from a consumer's report; the owning lane refines this file as it works.
