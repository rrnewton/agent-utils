---
title: 'agentctl-profile-masked-paths: keep launched agents out of owner-declared off-limits paths'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
- profiles
assignee: mu-maskpaths
created_at: 2026-11-23T13:29:03-05:00
updated_at: 2026-11-23T13:29:03-05:00
---

# Description

Agent sessions launched by agentctl must be unable to enter paths the owner declares off-limits, while the owner's own tools keep full access. Three recent agent sessions ran a build tool inside an owner-declared off-limits path (a large source checkout on these hosts) and left daemons behind; briefs alone did not hold. This issue adds an enforceable mask so a launched agent cannot see or enter those paths at all.

The real off-limits path is host-local configuration only. It never appears in this repository (code, tests, docs, issue files or commit messages); this issue and the implementation describe it generically as an owner-declared off-limits path.

# Design

1. Launch-profile key. A launch profile gains `masked_paths`, a list of absolute paths. A per-agent override recorded in the agent record wins over the profile value, following the `reasoning_effort` precedent from #1216 agentctl-effort-option. `agentctl status` shows both the declared masks (from the profile) and the effective masks (after the per-agent override). A profile without `masked_paths` behaves exactly as today. Masking is opt-in per profile until every supported harness passes the in-namespace checks in the Acceptance Criteria.

2. Mechanism. Each launch runs inside a nested user and mount namespace. The outer namespace bind-mounts an empty read-only directory over each masked path, then execs the harness through a second user-namespace step that restores the real uid and gid, so the harness runs with its real identity. Masking the target of a symlink covers the symlink. The Rust edition is primary (native `unshare(2)` and `uid_map` writes, or the equivalent `unshare` helper); the Python edition keeps behaviourally equal behaviour or explicitly refuses the key rather than silently ignoring it, following the repository parity rule.

3. Every launch path. Masking applies on every path that launches or relaunches a harness: `start`, `run-headless`, `revive`, `restore` (including `restore --boot`), `restart`, and the services turnover's headless restart. The effort-flag work in #1216 agentctl-effort-option touched the same set and is the map for finding each path. A relaunch uses the effective masks (per-agent override first, else the profile's current `masked_paths`).

4. Fail closed. If masking cannot be set up (namespace creation refused, a masked path cannot be covered, or the empty-directory mount fails), the launch refuses with a clear message and nothing is launched. There is no silent unmasked fallback.

5. Interim watch guard. Until masking is enabled on a profile, `agentctl watch` flags any process in a managed session's pane process tree whose working directory is under a masked path, with one notice per process rather than a flood.

# Acceptance Criteria

- A profile with `masked_paths` launches every harness (Claude Code, Codex, Muse) inside the namespace with each masked path covered by an empty read-only directory; the harness runs with its real uid and gid, and attempts to list, enter or build inside a masked path fail.
- Masking applies on `start`, `run-headless`, `revive`, `restore`, `restart` and the services turnover relaunch; a per-agent override in the record wins over the profile, and `agentctl status` shows the declared and effective masks.
- A launch whose masking cannot be set up refuses with a clear message and launches nothing; a profile without `masked_paths` launches exactly as before.
- Throwaway-session checks prove each harness still works inside the namespace: Claude Code, Codex and Muse (including its own sandbox), plus Herdr panes, ssh to GitHub through herdr-run, `git`, `cargo`, and an Eden-backed path where one is present. Any harness that breaks is recorded here and the key stays opt-in per profile until all pass.
- `agentctl watch` flags a managed session's process whose working directory is under a masked path, once per process.
- Tests are hermetic (a temporary directory stands in for the masked path, with no reliance on any real checkout) and use no wall-clock budgets. New test files are classified in `validation/components.json`.
- `common/docs/agentctl/harness-support-matrix.md` is updated in the same commit if per-harness support differs.

# Notes

[watermelon-full-preview] mu-maskpaths. Number pre-assigned by the coordinator. Prior work searched before filing: #1216 agentctl-effort-option (the launch-path map and the per-agent override precedent), #1023 (profile refresh on revive and restart), and the tracker and `ai_docs` for namespace and masking prior art; no existing issue covers launch-time path masking. The real off-limits path is set only in host-local profiles outside this repository; no real profile in `.agentctl/profiles.json` is enabled by this issue.
