---
title: 'revive-profile-env: a profile launch environment is lost on relaunch paths that bypass the recorded launch'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-12T12:20:00+00:00
updated_at: 2026-11-12T12:20:00+00:00
---

# Description

[watermelon-full-preview] On 2026-11-12 devbig014's lead revived 25 lanes after a 15-hour outage (#1405). The lanes launched from a profile running Muse at ultra effort came back at xhigh. That profile sets an environment variable that turns ultra on (`MUSE_EXPERIMENTAL_ULTRA_REASONING_EFFORT=on`; without it, Muse silently falls back to xhigh and the status bar says xhigh). `agentctl revive` replayed the argv, including the effort flag, but not the profile's environment. So a profile's `env` (or launch_env) is lost on at least one relaunch path.

Owner rule 319: effort is set intentionally by agentctl and the profiles on EVERY launch path (start, run-headless, revive, restore, restart, turnover), never by machine defaults. The same goes for anything else a profile declares.

Where the launch environment lives today: `start` records only environment NAMES in the agent record (`environment_names`) and in the launch spec (`launch_spec.environment_names`); values are never stored, because they stay in the owner's private profile. The registry-wide `launch_env` (env_file, path_prepend) is not recorded at all; it is read fresh from profiles.json at every `launch()`. The interactive revive paths re-read the recorded names' values from the launch profile and refuse when those names cannot be reconstructed, and an env-carrying revive never reuses the old pane (#1139). The gaps are the relaunch paths that do not go through that replay:

- `restart_with_resume` (the recovery ladder's child-first restart, called by the watcher) relaunches with `start_pane_agent` directly. It never calls `launch()`, so it applies neither the recorded profile environment nor the registry `launch_env` shell line. In a pane whose shell Herdr restored after an outage, the recorded environment is simply absent.
- The headless wrapper has no environment to replay: `WrapperConfig` carries no environment field, `spawn_harness` sets only `AGENTCTL_AGENT`, and a headless record always stores empty `environment_names` and no profile. `run-headless`, the headless `turnover` relaunch, `handback`, and a services-turnover unit restart therefore cannot re-apply any launch environment beyond what their own process happens to inherit.
- `refreshed_from_profile` rebuilds `environment_names` from the profile's `env` alone. It drops names agentctl itself synthesized at start (the Muse ultra gate added for an ultra effort) and any caller `--env` names, so a refresh followed by a revive can launch without the gate and fall back silently.
- `agentctl status` shows the recorded `environment_names`, but not the effective launch environment keys a relaunch would apply, so a lane whose recorded names are missing a profile-declared variable is not visible as missing.

# Design

Chosen rule: the RECORDED launch environment wins on every ordinary relaunch. A relaunch re-reads, from the private launch profile, the values for exactly the names the launch recorded (record `environment_names` / launch-spec `environment_names`, which include the ultra gate agentctl synthesized at start). The profile's CURRENT environment applies only through the existing explicit opt-in, `--refresh-profile` on revive/restart, which rewrites the record and launch spec first; when a record carries no recorded environment names and no launch profile, there is nothing to replay and nothing is invented. This follows the existing rule for which wins: under #1216 the recorded effective effort in the agent record overrides the profile and the profile is only a fallback when the record carries none, the launch-spec contract is that a profile edited after launch does not change what a revived agent runs, and #1023 already made the current profile an opt-in refresh rather than the default. The registry `launch_env` keeps its current semantics (read fresh from profiles.json at launch, typed into the pane shell after its startup files) on every path that launches through a pane.

- Every relaunch path applies the same environment `start` applied: revive and restore (which revives), restart (which revives), `restart_with_resume`, and the headless relaunch paths (run-headless, turnover, handback). Paths that cannot apply the recorded environment to the pane or process they relaunch into refuse with that reason instead of launching without it, as an env-carrying revive already refuses pane reuse (#1139).
- The headless record records its launch environment names like an interactive record does, and the wrapper spawns its harness with those entries (values re-read from the launch profile by name), so a wrapper relaunch, a turnover, a handback, and a unit restart all see the same environment the first wrapper launch saw. Headless profiles still may not declare `env` entries; this records what the launch itself carried, such as an agentctl-synthesized gate.
- `refreshed_from_profile` keeps agentctl-synthesized environment names that the current profile does not declare, instead of dropping them.
- `agentctl status` shows `launch_env_keys`: the sorted names of the effective launch environment keys a relaunch would apply (recorded names, any agentctl-synthesized gate for the recorded effort, and the registry `launch_env` contribution), names only, never values, in both editions where status exists.
- Rust is the primary edition. Python parity where the Python edition has these paths: its revive/restart replay and status. Python refuses `restore` (Rust edition only), has no same-pane `restart_with_resume` and no headless `turnover`, so those stay Rust-only and the Python edition keeps refusing them clearly.

# Acceptance Criteria

- A test per relaunch path (revive, restore, restart, `restart_with_resume`, and the headless turnover/wrapper relaunch) with a fixture profile that sets an env var, checking that the relaunched process or tab gets it, using stubbed herdr/harness with no live agents and no live registry.
- A Muse lane recorded at ultra whose gate came from the effort (not from the profile's `env`) still relaunches with `MUSE_EXPERIMENTAL_ULTRA_REASONING_EFFORT=on` after revive, and after a profile refresh that does not declare the gate.
- `agentctl status` shows `launch_env_keys` with names only; no environment value appears in status output.
- No relaunch path silently launches without a recorded environment it cannot apply: it applies it or refuses naming the reason.

# Notes

Numbered from devbig030's interim block (1400-1469). The devbig014 lead asked for it to be filed (2026-11-12). Related: #1023 agentctl-restart-profile-refresh, #1139 (env-carrying revive never reuses the pane), #1216 agentctl-effort-option (declared vs effective effort on every launch path).
