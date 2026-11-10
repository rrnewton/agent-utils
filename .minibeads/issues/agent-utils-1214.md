---
title: "wrkslots-soak-image-create-fallback: Rust soak refuses image-mode create without handing off to Python"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-10T15:05:40.000000000+00:00
updated_at: 2026-11-10T15:05:40.000000000+00:00
---

# Description

Reported by the devbig030 coordinator on 2026-11-10. On that host,
`worktrees/wrkslots create` run through the Rust soak wrapper for an
image-backed slot created nothing and its last line was
"slot-image provisioning stays with the Python edition in this slice".
The wrapper transcript did not capture an exit code. The Python CLI then
created the same two slots successfully.

A wrapper refusal with exit code 1 and that final line reads as a
near-silent failure: it neither performs the requested create in Python nor
names a fallback the caller can act on.

# Design

Reproduce image-mode create through `WRKSLOTS_ENGINE=rust-soak` with a
fixture registry and capture the wrapper's exit code, stdout, stderr, and
soak log. Then give the wrapper one explicit behaviour for image-mode
create and every refusal of the same "stays with the Python edition" kind:

- hand off to the Python edition for image-backed create; or
- fail nonzero with a clear error that names the fallback.

The preferred outcome is the handoff, because the soak header promises
commands outside Rust coverage run in Python with a stderr note. Preserve the
rule that a real Rust refusal after dispatch is final and is never silently
rerun in Python.

# Acceptance Criteria

- An image-mode create through the soak wrapper either creates the slot via
  the documented Python handoff or exits nonzero with a message naming that
  fallback; it never exits 0 with nothing created.
- The soak log records the edition that actually ran and the create result.
- A wrapper test covers image-mode create, including the exit code and
  stderr, against fixture state only.

# Notes

Fold the fix into the current Rust-primary landing stack after the issue is
reserved. The Python CLI is the working reference for image-backed create.
