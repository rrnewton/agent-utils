---
title: "wrkslots-machine-auto: let machine be absent or auto in a shared .wrkslots.yml, resolving to the short host name"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T20:55:00.000000000+00:00
updated_at: 2026-11-09T20:55:00.000000000+00:00
---

# Description

`.wrkslots.yml` carries a `machine:` value, written at init and required today. A project that tracks the file
(shared by every host that checks out the project) therefore names ONE machine for all hosts. On any other host,
wrkslots resolves the wrong machine identity unless `WRKSLOTS_MACHINE` is set in every environment that runs it.
A consumer hit this on 2026-11-09 while adopting #1152 worktrees-image-host-scope: the shared config names the
image host, so the other host's tools resolve as that host.

# Design

- `machine:` may be absent, or the literal `auto`. Either way the machine is the short host name
  (`socket.gethostname()` up to the first dot, the same default `--machine` documents).
- Precedence is unchanged: `--machine`, then `WRKSLOTS_MACHINE`, then the config value (where `auto` or absent
  means the host name).
- `init` keeps writing an explicit value for unshared projects, or offers `auto`; document which.
- `doctor` or `config` shows the resolved machine and where it came from.
- A shared tracked config can then drop `machine:` and pair it with the `machines:` list inside
  `worktrees_image` (#1152).

# Acceptance Criteria

- A config without `machine` resolves to the host name; `auto` does the same; an explicit value still wins; and
  `--machine` / `WRKSLOTS_MACHINE` still override all three. Tests cover each case.
- Existing configs with an explicit machine behave exactly as before.
- Help text and the USER_GUIDE describe absent/`auto`.
- Rust parity: the Rust config reader resolves the machine the same way.

# Notes

Requested by the lead coordinator for the worktrees-image set, right after #1152.
