# run-agent-prompt

Run one judgement-only prompt through the first agent harness that
answers (`muse`, then `claude`, then `codex`) and print only its
final answer. Each attempt is tool-free or confined to a read-only
sandbox, runs in a fresh temporary workspace by default, and has its
own timeout. The whole fallback chain also has a deadline.

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
