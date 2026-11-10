---
title: 'agentctl-effort-option: per-agent effort on start and run-headless, live set-effort, and real latency stamps'
status: open
priority: 1
issue_type: feature
assignee: mu-effort
created_at: 2026-11-10T15:16:46.000000000+00:00
updated_at: 2026-11-10T15:16:46.000000000+00:00
---

# Description

Owner request (reply 310, 2026-11-10 10:08 AM ET): "It seems like we need to add that option when starting an agent and we should probably have a way to change it along the way. I would actually like to switch that headless coordinator down from an extra high to high or even medium. The coordinator needs to be responsive."

Today a headless coordinator launched with `agentctl run-headless` has no effort option and stays at its harness default. Profiles can carry a `reasoning_effort` setting and `agentctl start` accepts `--reasoning-effort` for interactive and agentcloud launches, but `run-headless` takes no per-agent override, the new `--effort` spelling does not exist, and nothing changes the effort of a live agent. A coordinator that should drop from extra-high to high or medium for responsiveness cannot be switched without manual relaunch surgery.

Related prior work: #801 chat-latency-summaries and #897 chat-latency-stamps stamp model, effort, role and protocol on each chat request at delivery, and #1023 covers profile refresh on revive and restart. This issue is the missing launch and live-change surface those stamps should read.

# Design

Rust is the primary edition. Check whether the Python edition needs parity under the repository rules that follow #1200 rust-status-consistency; where parity is not required, the Python edition refuses the new options clearly rather than silently ignoring them, following the #1023 refresh-profile precedent.

1. Launch option. Add `--effort LEVEL` to `agentctl start` and `agentctl run-headless`, and to every path that re-launches an agent (revive, restart, restore). `--effort` and the existing `--reasoning-effort` are two spellings of one option on `start`; giving both with different levels is refused. The value is mapped per harness:
   - Claude Code: the `--effort` flag the installed `claude` CLI accepts (low, medium, high, xhigh, max), including headless `-p` sessions.
   - Muse: `--reasoning-effort`, including the ultra env gate: `ultra` also sets `MUSE_EXPERIMENTAL_ULTRA_REASONING_EFFORT=on` for the launch, as the muse-ultra profile does today.
   - Codex: the `model_reasoning_effort` config value.
   A level a harness does not support is refused with a clear error naming the harness and the levels it accepts; nothing is launched. The effective effort is stored in the agent record and overrides the profile's `reasoning_effort`, so restore, revive and `restart --resume` keep it.
2. Live change. Add `agentctl set-effort NAME LEVEL` (with `--urgent` as the only way to interrupt a mid-turn agent). It validates the level against the agent's harness, records the new level in the agent record at once, and applies it without interrupting a mid-turn agent: headless agents apply it from the next turn (or by a `--resume` restart at a safe point when the harness cannot take it live); TUI agents apply it at a safe point through the harness's own control (Claude Code `/effort` or `/model`, or the Muse equivalent where one exists). Where a harness has no live control, the command says so and offers `restart --resume`. The result reports the recorded level and how and when it takes effect.
3. Latency stamps. The #897 delivery stamp and the #801 latency profile read the recorded effective effort from the agent record (the field set by (1) and (2)), falling back to the profile only when the record carries none, so `chat latency --by effort` and `--by profile` show real levels instead of `unknown` for agents launched or switched under this issue. A genuinely unrecorded effort stays `unknown`, never guessed.
4. Declared effort on every relaunch (owner replies 318, 319 and 321, 2026-11-10). Effort is always intentionally controlled by the agentctl layer and its profiles (profile key: `reasoning_effort`), never by a harness or machine default (`~/.claude/settings.json`, `~/.codex/config.toml`, or a Muse default never decide it):
   - Every launch declares an effort: the profile's `reasoning_effort`, an explicit `--effort`, or the per-agent record override, and agentctl passes it explicitly to the harness on start, run-headless, revive, restore (including `restore --boot`), `restart --resume`, and the services turnover's headless restart. Tests cover each relaunch path: a declared effort from the profile, and a per-agent override in the record, is the effort the relaunched harness receives.
   - No declaration is a refusal (owner reply 321): when `start`, `run-headless`, or any launch would have no effort from the profile, from `--effort`, or (for run-headless) from the record, it refuses with a clear message naming the valid levels for that harness. There is no silent harness default and no agentctl-level fallback default. The coordinator choosing a lane chooses its effort (xhigh by default, ultra only for real parallel work, owner reply 261; see the agent-utils-coordinator skill's delegation steps).
   - A declared change takes effect without forcing a restart: `set-effort` on a headless agent without `--now` just records it, and the next natural restart (turnover, restore, revive, or a wrapper relaunch) picks it up. The first real use is a headless coordinator now running at high through a per-agent `--settings` file: its declared profile effort replaces that file at its next natural restart, operated by its owner.
   - Status and watcher (part 2): `agentctl status` shows the declared effort and the effective effort read from the harness's own report (the stream-json init event, or the Muse status bar), and the watcher flags a mismatch between them, like the known Muse gate case where ultra falls back to xhigh.
   Delivery is split: part 1 is the launch option, the live record change, the refusal, and the latency stamps; part 2 is the status/watcher mismatch report.

# Acceptance Criteria

- `agentctl start NAME --effort high` and `agentctl run-headless NAME --effort high` launch at that effort for each supported harness, mapped as designed above; `--reasoning-effort` keeps working on `start` and agrees with `--effort` when both are given.
- An unsupported level for a harness is refused before launch with an error naming the harness and its accepted levels.
- The agent record carries the effective effort, the profile value is overridden, and revive, restore and `restart --resume` relaunch at the recorded effort.
- Muse `ultra` launches with the ultra env gate set; a non-ultra level does not set it.
- `agentctl set-effort NAME LEVEL` records the new level, never interrupts a mid-turn agent without `--urgent`, applies headless changes from the next turn, applies TUI changes at a safe point where the harness supports it, and otherwise reports that a `restart --resume` is needed.
- Chat latency records for an agent with a recorded effort stamp that effort, so grouping by effort shows it; an agent with no recorded effort still stamps `unknown`.
- Help for `start`, `run-headless` and `set-effort` describes `--effort`/`LEVEL`, its defaults and its per-harness applicability, with an example.
- Focused tests cover the per-harness mapping and refusal, the record persistence across a resume relaunch, the mid-turn refusal without `--urgent`, and the latency stamp reading the recorded effort.

# Notes

Owner reply 310, 2026-11-10 10:08 AM ET, via the lead coordinator. The tracker was searched first: #801, #897, #946, #975, #1023 and #1200 cover adjacent ground; no existing issue covers a launch-time or live effort option. Switching any particular live coordinator's level is operated separately by its owner once this lands; this issue builds only the option.
