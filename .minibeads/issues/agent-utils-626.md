---
title: 'crates-publish-prep: get the Rust tools ready for crates.io and propose a first release'
status: open
priority: 2
issue_type: task
created_at: 2026-10-28T13:03:15.268381551+00:00
updated_at: 2026-10-28T13:03:15.268381551+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 08:52 ET: start publishing agent-utils tools to crates.io, bump versions just before the release, eventually `cargo install vibe-talk`; asked whether the name agent-utils is free and whether a meta package could install the whole suite.

Preparation only: no real `cargo publish` until the owner approves an exact crate list. Deliverables: per-crate readiness (cargo package / publish --dry-run, metadata, path dependencies, embedded assets, size, private references), publish names with alternatives for taken names, a meta-package design, the release procedure, and the proposed first-release list. Design note: ai_docs/transient/2026-10-28-crates-publish-prep.md.
