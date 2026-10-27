---
title: 'cargo-runner-cachedir-tag: refresh refuses an untagged rs/target silently'
status: closed
priority: 0
issue_type: bug
labels:
- rust
- launcher
created_at: 2026-10-27T08:10:04.266167077+00:00
updated_at: 2026-10-27T08:46:34.008170177+00:00
closed_at: 2026-10-27T08:46:34.008170027+00:00
---

# Description

When a tool's cached artifact is missing, rs/bin/cargo-runner runs `cargo clean --quiet --release --package <pkg> --target-dir rs/target`. Cargo refuses to clean an existing target dir without a valid CACHEDIR.TAG ("missing or invalid CACHEDIR.TAG ... aborted to prevent accidental deletion"), exits 101, and --quiet suppresses the message, so the launcher reports only "Cargo clean failed" and exits 3.

Cargo writes CACHEDIR.TAG only into a target dir it creates itself. The Rust validation suites build into rs/target/validation-tests, so in a checkout without rs/target that run creates rs/target untagged.

Fix: never pass --quiet to cargo clean and print its output on failure; before cleaning, tag the launcher's own untagged rs/target, and when rs/target cannot be tagged (a symlink, an unwritable tree) remove that package's release artifacts directly; and have scripts/case_timings.py run-cargo tag a --target-dir parent it has to create.
