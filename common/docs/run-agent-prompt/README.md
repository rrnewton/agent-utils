# run-agent-prompt

Run one judgement-only prompt through the first agent harness that
answers (`muse`, then `claude`, then `codex`) and print only its
final answer. Muse and Claude Code attempts are tool-free; Codex
attempts are read-confined with their shell and MCP surfaces
disabled. Each attempt runs in its own fresh temporary workspace by
default and has its own timeout. The whole fallback chain also has a
deadline.

Install from the package index:

```sh
python3 -m pip install run-agent-prompt
```

Python 3.10 or newer and at least one installed harness are required.

```sh
run-agent-prompt --prompt-file prompt.txt
cat prompt.txt | run-agent-prompt --only claude
```

Stdout contains only the winning answer. Exit 0 means a harness
answered, 69 means every harness failed, and 2 means a usage or
config error. Attempt results are logged to stderr without prompt or
answer text.

Run `run-agent-prompt quickstart`, `run-agent-prompt --userguide`, or
`run-agent-prompt --help` for the operator contract.
