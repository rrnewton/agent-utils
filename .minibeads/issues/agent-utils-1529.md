---
title: 'vibe-talk-prompts-command: vibe-talk prompts --hours K prints the owner record in one go'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T00:00:00.000000000+00:00
updated_at: 2026-11-14T00:00:00.000000000+00:00
---

# Description

Make it very easy for an agent to use the vibe-talk CLI to get a record of the owner's prompts all in one go in the last K hours, idiomatically, reminding the agent that this is the single source of the owner's intent and requests. Today an agent must source the instance's environment and pass config and overlay paths by hand. An all-channel upstream recap can also hit the provider rate limit, so the new command must read only the local message cache and make no provider API calls; where the cache may be incomplete it must say so and name the threads, never fetch upstream.

# Design

New subcommand `vibe-talk prompts --hours K` (default 24; also `--since <time>`, `--channel`, `--json`, `--cache`). Configuration discovery: an explicit flag or environment variable wins, else a documented order under the user's config dir (`app.toml`, `vibe-talk.toml`, `config.toml` under `$XDG_CONFIG_HOME/vibe-talk` or `~/.config/vibe-talk`, then `./vibe-talk.toml`); the error names every path looked for when nothing is found. No host names are hard-coded. The owner is the configured owner user ids (`[owner] user_ids` plus each provider's `owner_user_id`). The cache sqlite is opened read-only; owner messages in the window print oldest first with ET times, channel, thread id, and message id (the cache stores no permalinks, and the bridge request number lives in the bridge's own state, not this store, so neither is cheaply joinable here). `--channel` accepts the name with or without `#` as well as the id; the same matching now serves `recap` and `search`. Output opens with the reminder line. Cache-completeness warnings name threads whose recorded reply count exceeds their cached replies, partial rows, and channels with a dropped update in the window.

# Acceptance Criteria

- `vibe-talk prompts` with no flags on an installed host finds its config, overlay and cache itself.
- Only the owner's messages print, thread replies included, oldest first, ET-labelled, each with channel and thread reference.
- Cache-only: no server and no provider API calls are made.
- Tests against a fixture cache store with a fixed window: owner-only filter, thread replies of a pre-window root, order, ET labels, channel name matching with and without `#`, discovery precedence. No wall clock.
- Docs: a vibe-talk README section and CLI help, plus a one-line pointer in the coordinator skill.

# Notes

Owner tooling request 058, second of two (after #1528). The bridge request number is not included: it lives in the bridge's state, not vibe-talk's store, and is not cheaply joinable from a cache-only read. No consumer names, host names or local paths are recorded here.

## Resolution

[watermelon-full-preview] Added `vibe-talk/src/prompts.rs` (parse/discover/read/render, injected `now` via `run_at`), wired as the `prompts` subcommand in `main.rs`, shared `channel_matches` also used by `recap` and `search`. Fixture-store tests in `vibe-talk/tests/prompts.rs` plus in-module parse/matching tests pass; README section and the coordinator-skill pointer added.
