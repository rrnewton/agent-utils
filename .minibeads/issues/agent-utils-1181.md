---
title: "gate-soak-scoped-host-optin: scope the per-host Rust readers opt-in to named projects"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-10T02:14:51.000000000+00:00
updated_at: 2026-11-10T02:14:51.000000000+00:00
---

# Description

The Rust readers soak can be opted in per process with
`WRKSLOTS_RUST_READERS=1` or per host with an `enabled = true` line in
`~/.local/etc/agent-utils/wrkslots-rust-readers`. The host form currently
applies to every project whose `bin/wrkslots` goes through
`engine-resolver`. On devbig014, that includes another consuming project's
agent-utils checkout, so turning the soak on for the development harness
would also switch that other project's reader calls to Rust without its
coordinator's consent.

# Design

Version the host file format so old resolvers stay off. Remove the bare
`enabled = true` trigger and replace it with:

- a `readers = scoped` line; and
- one or more `project = /absolute/project/root` lines.

The resolver determines the request's project root from an explicit leading
`--project-root` if present, otherwise from the nearest ancestor containing
`.wrkslots.yml`, canonicalizes it, and honors the file only on an exact listed
match. `WRKSLOTS_RUST_READERS=1` remains a deliberate per-process opt-in for
any project; `WRKSLOTS_RUST_READERS=0` still wins. When a scoped file exists
but the requested project does not match, log one line under the existing soak
diagnostics saying why Python was chosen. The user guide states that the
legacy `enabled = true` form is ignored on purpose. Gate children remain
isolated by the #1170 environment guard.

# Acceptance Criteria

- A scoped host file routes an approved reader to Rust only when the resolved
  project root exactly matches a listed `project =` path.
- The same scoped file leaves an unlisted project on Python and logs why.
- A legacy `enabled = true` file never enables routing, including through an
  older resolver that only recognizes that key.
- Explicit `--project-root` and discovered `.wrkslots.yml` roots agree, and an
  unknown project root does not enable routing.
- `WRKSLOTS_RUST_READERS=1` still enables per-process routing and
  `WRKSLOTS_RUST_READERS=0` still disables it.
- Resolver tests cover listed, unlisted, legacy-file, explicit-env, and
  opt-out cases; the user guide documents the format and migration.

# Notes

Approved by the lead coordinator on 2026-11-09 before the reader soak is
restarted on devbig014. The first listed project is
`/home/newton/work/dev-agent-utils`.
