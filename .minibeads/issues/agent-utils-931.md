---
title: 'merge-queue-transient-ssh-push: SSH/proxy transport errors on the landing push are transient, Stop logs at once and never re-leases'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T12:04:31.622103030+00:00
updated_at: 2026-11-06T12:04:31.622103030+00:00
---

# Description

A consuming queue: SSH/proxy banner failure on landing push ended run with no retry; run parked behind slowest in-flight validation before recording stop; refused donor job re-leased after stop decision.

# Design

One push-failure classifier transient/permanent/unknown shared with GH013 item. SSH/proxy transport errors retried with push_retries/backoff. Stop logs stopped event at once, cancels/detaches in-flight, refused job never re-leased after stop, status/heartbeat say stopping.

# Acceptance Criteria

Fake remote first push SSH banner fail second succeeds: one push-retry then landed. Head Stop with validation in flight: stopped event within one cycle, in-flight cancelled/detached, prompt end, later green kept. COULD_NOT_RUN after stop not re-leased.
