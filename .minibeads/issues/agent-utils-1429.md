---
title: 'launch-required-env: refuse a launch that lacks a required environment variable'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-14T20:22:21+00:00
updated_at: 2026-11-14T20:22:21+00:00
---

# Description

[muse watermelon] Some sites must launch a harness only with specific environment variables set, for example a variable selecting a mandatory gateway. Launch profiles carry that environment (the profile `env`). When a team's profiles were refused for an unrelated reason (since fixed), they fell back to `agentctl start NAME --harness codex --effort xhigh` with no profile. That launched the harness WITHOUT the profile's environment, so it silently routed to a default gateway instead of the mandatory one. Three lanes ran that way before anyone noticed.

# Design

- The registry (`.agentctl/profiles.json`) declares required launch environment per harness with a top-level `required_env` object, for example `"required_env": {"codex": ["EXAMPLE_GATEWAY"]}`, so a profile-less `--harness X` launch cannot bypass it. A profile may add its own `required_env` list for its harness.
- agentctl refuses any launch for that harness whose launched environment lacks a required variable or has it empty. The refusal names the missing variable(s), which config declared them (the registry `required_env` or a named profile), and a profile that sets them when one exists. There is never a silent fallback.
- The check runs on every launch path: `start` (interactive and headless), `run-headless`, `revive`, `restore`, `restart`, and turnover relaunch, in both editions (`rs/agentctl` and `py/agentctl`), following the launch-path enumeration of #1216 agentctl-effort-option.
- Registries with no requirement behave exactly as today. Documentation, tests, and fixtures use neutral variable names such as `EXAMPLE_GATEWAY` only.

# Acceptance Criteria

- A registry declaring `"required_env": {"codex": ["EXAMPLE_GATEWAY"]}` refuses a profile-less Codex launch without `EXAMPLE_GATEWAY`, naming the variable, the registry declaration, and a profile that sets it.
- The same launch with the variable set (in the caller's environment, a literal `--env` entry, or the selected profile's `env`) proceeds, on every launch path in both editions.
- A profile's own `required_env` list adds requirements for launches using that profile.
- Tests in each edition cover refusal, satisfaction, empty-value refusal, and backward compatibility; CLI help and the userguide/quickstart describe the setting with a neutral example.

# Notes

Reported after three lanes ran against a default gateway instead of the mandatory one.

## Resolution

[muse watermelon] Both editions now parse a top-level `required_env` object (harness to variable names) and a per-profile `required_env` list in `.agentctl/profiles.json`, validated as unique well-formed variable names. `check_required_env` (Rust `crate::profiles::check_required_env`, Python `agentctl.profiles.check_required_env`) refuses a launch when a required variable is absent or empty in the effective environment (the caller's environment overridden by literal launch entries), naming each missing variable, its declaring config, and a profile that sets the missing names when one exists. The check is wired into every spawn path: Rust `start_inner`, `start_headless`, `run_headless_with_effort`, revive's `finish_launch_plan` (which restart, restore, and revive share), and `turnover_headless`; Python `ManagedAgents.start`, the headless branch of `Sessions.start_session`, and revive's `_policy` (which restart shares). CLI `--profile` help and the userguide/quickstart in both editions document the setting with the neutral `EXAMPLE_GATEWAY` example. Tests: `rs/agentctl/src/subagents/tests/required_env.rs` (check semantics, bad shapes, profile-less refusal with profile suggestion, profile launch) and `py/tests/test_agentctl_sessions.py` (parsing, check semantics, file-backed registry/profile resolution, bad shapes, and a start that refuses before launch and proceeds once the variable is set).
