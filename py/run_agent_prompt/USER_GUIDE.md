# run-agent-prompt

`run-agent-prompt` solves one boot-time problem: a recovery shepherd
needs an agent's judgement when the usual agent machinery is exactly
what may be broken. The outcome is a single answer from whichever
harness still works, with no dependency on Herdr, agentctl sessions, or
a TUI. It depends only on Python 3 (stdlib) and whichever of `muse`,
`claude`, and `codex` are installed.

## quickstart

```sh
run-agent-prompt --prompt-file prompt.txt
cat prompt.txt | run-agent-prompt --timeout 120
run-agent-prompt --only claude --claude-model MODEL --prompt-file prompt.txt --log-file attempts.jsonl
```

The prompt comes from `--prompt-file PATH` or stdin and never appears
in argv. Exit 0 means stdout holds exactly the winning harness's final
answer; exit 69 means every harness failed; exit 2 is a usage or
config error. stderr gets one line per attempt (`harness=`, `status=`,
`exit=`, `duration_s=`, `model=`, `effort=`) and never prompt or answer
text; `--log-file PATH` appends the same fields as JSONL.

## userguide

Attempts run in the order muse, claude, codex unless `--only` names
one. An attempt fails on spawn error, timeout, nonzero exit, an invalid
effort for that harness, or a whitespace-only answer; the next harness
is then tried. `--timeout SECONDS` (default 300) bounds one attempt:
the attempt's whole process group is terminated, given two seconds,
then killed. `--total-timeout SECONDS` (default 900) bounds the whole
chain. Every attempt is judgement-only by construction — muse with an
empty preset and shell/write/web tools disabled, Muse with an empty
tool list, Codex in a read-only sandbox — because recovery effects
belong to `agentctl team act`, never to an agent's own tools.

Each attempt runs in a fresh temporary workspace unless `--cwd DIR`
opts into a caller-chosen directory, so a judgement call cannot read or
write the caller's repository. Children inherit only an allowlisted
environment (paths, locale, XDG, proxy/cert variables, and the
harnesses' auth variables); values are never logged.

`--effort LEVEL` (default from config, else `low`) is translated per
harness, and `--muse-effort`, `--claude-effort`, and `--codex-effort`
override it per attempt. Models come from `--muse-model`,
`--claude-model`, and `--codex-model`, else from the optional config
file `~/.config/agent-utils/run-agent-prompt.toml` (`--config PATH` to
name another):

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

An unset model runs the harness default; the model the harness
actually reports using is recorded in its attempt line, otherwise
`unknown`. The per-attempt executables can be replaced with
`--muse-command`, `--claude-command`, and `--codex-command`
(primarily for tests and wrappers). A Codex `first_prompt`-style empty
result usually means its wrapped instruction/context messages filled
the bounded answer channel; the attempt is recorded `empty`.
