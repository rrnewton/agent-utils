---
title: "worktrees-image-host-scope: the worktrees_image section in the shared .wrkslots.yml applies to every host that checks out the project"
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T17:55:00.000000000+00:00
updated_at: 2026-11-09T17:55:00.000000000+00:00
---

# Description

`worktrees_image` is configured in `.wrkslots.yml`, and a project usually tracks that file, so every host that
checks out the project reads it: lane hosts and donor tool trees alike. The first adopter of the worktrees image (a
consumer) committed the section for the one host where the image exists. Within half an hour a donor on another
host refused a merge-queue job with `cannot create a slot: worktrees image is not-mounted`. The adopter reverted
the commit and now keeps the section as an uncommitted local edit on the image host, which is fragile: any reset,
fresh clone or checkout drops it.

A host-specific storage choice has no host-scoped place in the configuration.

# Design

Pick one, or both:
- A host-scoped form inside the section, for example `worktrees_image: {machines: [host-a], ...}`. A host whose
  machine name is not listed behaves as if the section were absent. Use the same machine identity wrkslots already
  records in slot rows (`--machine`, default the host name).
- A host-local overlay file that is never tracked, such as `.wrkslots.<machine>.yml` beside the main file or a
  file under the control directory, merged over the shared file for that host only, with documented precedence and
  a refusal for keys that must not differ between hosts.

Validation must refuse a malformed `machines` list. `wrkslots config` (or `doctor`) should say whether the section
applies on this host, and why.

# Acceptance Criteria

- On a host not named (or without the overlay), every command behaves exactly as if `worktrees_image` were absent:
  create, remove, status, migrate and config load. A test covers it.
- On the named host, behaviour is unchanged (existing worktrees-image tests pass).
- The USER_GUIDE and `--help` describe the host-scoped form and the overlay precedence.
- Rust parity filed or done (the Rust config reader must agree on which hosts the section applies to).

# Notes

Reported by the first adopter on 2026-11-09. It blocks that adopter from committing its configuration, so it runs
right after #1147 worktrees-image-bind-record-race in the worktrees-image set.

## Resolution

Shipped the host-scoped form only (the overlay file can follow if asked): an optional `machines` list inside the `worktrees_image` section names the hosts (the configuration's own machine identity) the whole section applies on. `settings_from_obj` validates it on every host (non-empty, unique, non-empty strings; malformed refuses everywhere) and `_parse_worktrees_image` is the one applies-here decision: an unnamed host gets `worktrees_image = None` exactly as if the section were absent, with the reason kept on the config for diagnostics. Tests prove config load on an unnamed host attempts no mount or bind at all, the create guard does not refuse, `doctor` reports `worktrees-image: skipped: ...` (human and JSON) while reporting a named host's slot row plainly, migrate's refusal names the host scope, and the named host is unchanged (existing tests pass). Documented in the section's configuration comments, `worktrees-image --help`, and the USER_GUIDE ("Host-scoped sections"). Rust parity: mirror the `machines` validation in the Rust settings and config-validation readers and the applies-here decision where Rust builds its config (filed with the landing PUSH).
