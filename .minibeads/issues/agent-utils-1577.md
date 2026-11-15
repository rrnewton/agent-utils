---
title: 'doctor-recovery-commands: emit the exact terminal command per divergence'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Status and doctor already type every
registry/storage divergence (row-without-directory,
directory-without-row, git-registration-without-row, create
journals), and audit already computes `record_sha256`, but none
emits the ready-to-run recovery command. Five absent rows and one
interrupted create waited days for commands that already exist
(`recover-absent-*`, `recover --slot/--abort-create`).

# Design

Per typed finding, doctor/status JSON gains a `remedy_command`
with exact slot, generation, and digest filled in (human format
prints it too). Directory-without-row is classified (git checkout,
cache, plain directory) with its matching terminal command or an
explicit inspect-first verdict. Commands stay proposals: doctor
authorizes nothing.

# Acceptance Criteria

- Each divergence fixture emits a command that, run verbatim on
  the fixture, reaches its terminal state or refuses by name.
- No remedy is emitted where preconditions cannot be established
  (the finding says what to inspect instead).
