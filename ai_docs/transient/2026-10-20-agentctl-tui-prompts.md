# agentctl-tui-prompts: Codex non-interactive investigation

Checked October 20, 2026, with the installed Codex CLI 0.163.0, its real
`codex --help`, `codex exec --help`, and read-only `codex features list`.
No Codex session was launched and no profile or user configuration was changed.

- `codex exec` is explicitly the non-interactive command. The official
  [non-interactive guide](https://developers.openai.com/codex/noninteractive)
  says it runs without opening the interactive TUI. It is the documented way to
  avoid terminal question boxes in an automation run; it is not a switch to an
  already running interactive goal session.
- Neither installed help surface advertises a flag specifically disabling
  `request_user_input` or goal-run questions. The official
  [configuration reference](https://developers.openai.com/codex/config-reference)
  has no `request_user_input` entry found by exact search.
- `--ask-for-approval never` controls command approvals. Its help and the config
  reference do not say it suppresses model clarification questions; do not treat
  it as a solution to this problem.
- Installed `codex features list` reports `default_mode_request_user_input` as
  `under development` and `false`. The generic help supports `--disable FEATURE`,
  but this is not evidence of a supported all-mode or goal-specific no-question
  setting. It does not establish behavior for asynchronous questions. No such
  configuration is recommended or silently applied here.

Question forms are tested against upstream rendered Codex snapshots, pinned
with their source commit and provenance in the shared test fixture README.
The portable test suite drives mock terminals only. Live harness behavior beyond
those captured layouts remains unverified; unfamiliar, remapped, truncated,
multiple-question, or nonempty forms are refused rather than guessed.

Owner clarification, October 20, 2026: inspection should reveal a collapsed
question, without selecting or submitting an answer. This supersedes the initial
strict no-keystroke inspection interpretation. Both editions now use only the
adapter's reveal keys for `prompts`, under the same pinned-recipient and pane
input locks as answering. Expanded forms receive no input. Paused sessions may
be revealed for inspection, while automated answers remain paused.
