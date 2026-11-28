# Terminal form fixtures

Codex question snapshots are upstream rendered screen captures from OpenAI Codex
commit `66d0de9983e82b266e89ee29523324c9d08a5472`,
`codex-rs/tui/src/bottom_pane/request_user_input/snapshots/`, licensed under [Apache-2.0](CODEX-LICENSE)
(Copyright OpenAI). They retain the upstream text after removing the Insta metadata header and
trailing terminal padding; no question, option, or footer text is changed. Source: https://github.com/openai/codex/tree/66d0de9983e82b266e89ee29523324c9d08a5472/codex-rs/tui/src/bottom_pane/request_user_input/snapshots

The compact state corpus is a reduced terminal-rendering fixture, not a claim
of a live run against either harness. It covers both supported adapters and
contradictory/stale screen markers. No live agent was inspected or driven.

`claude-quoting-muse-peer.txt` is a live Claude Code pane read back with
`agentctl read` on 2026-11-27 (#2421): a tool output quoting the Muse
peer-dialog detector's doc comment (a `Message from <peer>` heading and all
four choices) above a running turn. Rows between the quoted block and the
spinner were dropped so the quote sits in the bottom 24 rows, as it did when
a running watcher reported the pane at a peer prompt; no row text changed.
