# run-agent-prompt quickstart

`run-agent-prompt` asks one judgement-only question of the first
working agent harness. It tries `muse`, then `claude`, then `codex`,
and prints only the final answer.

Install it with `python3 -m pip install run-agent-prompt`. Python
3.10 or newer and at least one installed harness are required.

```sh
run-agent-prompt --prompt-file prompt.txt
cat prompt.txt | run-agent-prompt --timeout 120
run-agent-prompt --only claude --prompt-file prompt.txt
run-agent-prompt --profile-file recovery-profiles.toml --prompt-file prompt.txt
```

The prompt is read from `--prompt-file PATH` or stdin and never
appears in argv. Each attempt runs in its own fresh temporary
workspace unless `--cwd DIR` selects another directory. Exit 0 means stdout holds the
winning answer; exit 69 means every harness failed; exit 2 means a
usage or config error.

Run `run-agent-prompt --userguide` for models, effort, config,
timeouts, logging, and the child-environment contract.
