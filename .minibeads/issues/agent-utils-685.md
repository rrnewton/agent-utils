---
title: 'agent-env-path: put each team''s own agent-utils tools first on every agent PATH and retire the global copies'
status: open
priority: 2
issue_type: feature
created_at: 2026-10-29T12:46:35.730438477+00:00
updated_at: 2026-10-29T12:46:35.730438477+00:00
---

# Description

[opus 5.5] Agents on a shared host resolve suite tools (agentctl, agent-teams, mb, herdr-agent)
through user-level global copies that come first on every agent's PATH, so a team cannot be sure
that its agents run the agent-utils version it pins or develops. Research and tested recipes:
common/docs/agent-harness-environment.md.

Follow-up migration work:

1. agentctl: a per-registry launch environment (for example a `launch_env` or `env_file` key in
   profiles.json) that is typed as part of the harness command line
   (`env BASH_ENV=<file> PATH=<team bin>:"$PATH" <harness> ...`), so it applies after the pane
   shell's rc files and on both terminal backends. For Codex, also `-c allow_login_shell=false`.
2. Build provenance: `agentctl --version` and `capabilities` report the source commit. Today every
   build prints `agentctl 0.1.0` and `source_fp: null`.
3. Startup staleness check: compare the embedded commit with the team checkout's HEAD (and
   optionally the parent's gitlink) by reading files only, then warn, or refuse when asked to.
4. Launcher fast path: a stat-manifest check (path, size, ctime of rs/) before the
   content-verified path. Prototype measured 9 ms per call against 0.43-0.47 s today, where five
   source fingerprints cost about 285 ms.
5. Per-team built binaries keyed by commit (`<team>/.agent-utils-bin/<commit>/`, `current` link),
   rebuilt by the pin bump.
6. Final step: remove the global copies once every consumer has a replacement path.

Acceptance: every agent of a team resolves `agentctl` into that team's bin dir; `--version` names
the commit; no user-level global suite copy remains.
