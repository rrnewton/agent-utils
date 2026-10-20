# Terminal form fixtures

Codex question snapshots are upstream rendered screen captures from OpenAI Codex
commit `66d0de9983e82b266e89ee29523324c9d08a5472`,
`codex-rs/tui/src/bottom_pane/request_user_input/snapshots/`, licensed under [Apache-2.0](CODEX-LICENSE)
(Copyright OpenAI). They retain the upstream text after removing the Insta metadata header and
trailing terminal padding; no question, option, or footer text is changed. Source: https://github.com/openai/codex/tree/66d0de9983e82b266e89ee29523324c9d08a5472/codex-rs/tui/src/bottom_pane/request_user_input/snapshots

The compact state corpus is a reduced terminal-rendering fixture, not a claim
of a live run against either harness. It covers both supported adapters and
contradictory/stale screen markers. No live agent was inspected or driven.
