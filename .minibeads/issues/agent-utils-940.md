---
title: 'gate-cargo-target-prune: prune cargo target artifacts that the latest gate build did not touch'
status: open
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T14:30:00.000000000+00:00
---

# Description

Every lane slot keeps its own cargo targets (rs/target and vibe-talk/target), and they grow a few GiB per gate as stale hashed test binaries and incremental dirs pile up. On one host, six live slots held about 125 GiB of targets, and one slot's rs/target went from 5 to 9 GiB in about 6 hours. With many gates an hour, this is the main steady driver of disk use.

# Design

After each gate (or as a gate.py post-step), prune target artifacts that the latest build did not touch: mtime-based, in the spirit of cargo-sweep, keeping everything the just-finished build used. Report bytes before and after. Also evaluate a shared target dir (or a shared build cache) for gate trees, given cargo's target-dir lock.

# Acceptance Criteria

- A gate run reports target bytes before and after the prune; a second identical gate does not rebuild anything the prune removed.
- Stale artifacts from earlier source versions are removed.
- The shared-target option is evaluated with numbers (disk saved against lock contention) in the issue.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.
