# run-agent-prompt user guide

`run-agent-prompt` obtains one plain-text judgement from an installed
agent harness when the caller needs an answer but must not give that
agent tools or recovery effects. It tries `muse`, then `claude`, then
`codex`, and stops at the first non-empty final answer.

Install the command with:

```sh
python3 -m pip install run-agent-prompt
```

Python 3.10 or newer is required. On Python 3.11 and newer the
wrapper uses only the standard library; on Python 3.10 its declared
`tomli` dependency supplies the same TOML parser API. At least one of
`muse`, `claude`, or `codex` must be installed and able to answer in
its non-interactive mode.

## Basic use

```sh
run-agent-prompt --prompt-file prompt.txt
cat prompt.txt | run-agent-prompt --timeout 120
run-agent-prompt --only claude --claude-model MODEL --prompt-file prompt.txt
run-agent-prompt --prompt-file prompt.txt --log-file attempts.jsonl
```

The prompt comes from `--prompt-file PATH` or stdin. It never appears
in the wrapper's argv. For Muse it is passed in a temporary prompt
file; for Muse and Codex it is written to the child's stdin.

Stdout contains only the winning harness's final answer. The exit
codes are:

- `0`: a harness answered.
- `2`: a usage or config error.
- `69`: every selected harness failed.

Every attempt also produces one stderr line with `harness=`,
`status=`, `exit=`, `duration_s=`, `model=`, and `effort=`. A failed,
timed-out, empty, or unstartable attempt may add one bounded,
sanitized `detail=` excerpt from harness stderr. Prompt fragments in
that excerpt are redacted. `--log-file PATH` appends the same fields
as one JSON object per attempt. Neither stderr nor the JSONL log
contains the answer.

## Attempts and time limits

Attempts run in Muse, Muse, Codex order unless `--only HARNESS`
selects one. An attempt falls through to the next harness when the
executable cannot start, the effort is unsupported, the child times
out, the child exits nonzero, or its final answer is empty.

`--timeout SECONDS` bounds one attempt and defaults to 300 seconds.
On timeout the attempt's whole process group is terminated, given
two seconds to exit, and then killed. `--total-timeout SECONDS`
bounds the whole chain and defaults to 900 seconds. Later attempts
receive only the time remaining under that chain deadline.

Attempts are judgement-only:

- Muse runs `muse exec --json --preset empty` with shell, write,
  and web tools disabled.
- Muse runs in print mode with `--tools ""`, safe mode, and
  no session persistence.
- Codex runs `codex exec --ephemeral --sandbox read-only`; its final
  answer is read from the last-message file in wrapper-owned scratch
  space.

Recovery effects therefore remain the caller's separate,
deterministic responsibility.

## Workspace and environment

By default each call creates a fresh temporary workspace and removes
it when the chain ends. `--cwd DIR` opts into a caller-selected
workspace. Codex's last-message file and Muse's prompt file stay in
wrapper-owned scratch space even when `--cwd` is used.

Children receive an allowlisted environment rather than the caller's
whole environment. The allowlist covers path lookup and home
directories, locale and XDG settings, proxy and certificate
variables, and the authentication variables the three harnesses use.
Environment values are never logged.

## Models, effort, and config

`--effort LEVEL` supplies a shared reasoning effort. The per-harness
flags `--muse-effort`, `--claude-effort`, and `--codex-effort`
override it. `--muse-model`, `--claude-model`, and `--codex-model`
select models per harness.

When a flag is absent, values come from the optional config file at
`~/.config/agent-utils/run-agent-prompt.toml`. Use `--config PATH`
to select another file. Every key is optional:

```toml
effort = "low"

[muse]
model = "..."

[claude]
model = "..."

[codex]
model = "..."
effort = "high"
```

Explicit flags override per-harness config values, which override
the shared config effort. If no effort is configured, `low` is used.
If no model is configured, the harness default runs. Where the
harness reports the model it actually used, that model is recorded
in the attempt line and JSONL record; otherwise the model is logged
as `unknown`.

The executable for an attempt can be replaced with
`--muse-command`, `--claude-command`, or `--codex-command`. These
flags accept a command line and are primarily for tests and local
wrappers.

## Manual harness check

The release check on 2026-11-21 used a prompt that asked the harness
to create `TOOL_CHECK` in an explicit temporary `--cwd` directory:

- Muse Code `1.4.25 (1.4.25-R9199.1)` answered
  `NO_TOOL_USED`; no `TOOL_CHECK` file was created.
- Claude Code `2.1.319` answered `NO_TOOL_USED`; no `TOOL_CHECK`
  file was created.
- Muse `0.170.0` was launched and created no `TOOL_CHECK`
  file, but its configured provider rejected the request before an
  answer with `Error: Daybreak requires the OpenAI model provider or
  a provider configured for cyber access programs`. The wrapper
  reported that detail and returned 69 for `--only codex`.
