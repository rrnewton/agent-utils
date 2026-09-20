# agentctl

Keep several coding agents working on a project, send them follow-up tasks, and
open their terminals whenever you need to inspect or steer them. A person or a
coordinator agent uses the same named-session interface.

```sh
agentctl quickstart
agentctl start reviewer --cwd . --brief 'Review the current changes'
agentctl send reviewer 'Focus on cancellation and restart behavior'
agentctl attach reviewer
```

Interactive sessions keep the native Codex or Claude terminal running in
[Herdr](https://github.com/herdrdev/herdr), installed separately. Authenticate
and install the chosen harness separately too. The command stores session
identity and durable delivery state locally; it needs no hosted account or
always-running manager process.

Installations with the worker and Chat extensions also provide resumable
headless turns in Herdr or tmux, a Google Chat coordinator bridge with reaction
ACKs and automatic capture of explicitly tagged replies, and an MCP
interface. The agent can send progress updates and multiple replies to one
request from its terminal without writing a reply file. Its reply tags are
independent of the chat provider.
Thread replies include a command hint for reading earlier context.
Run `agentctl capabilities` to see the modes,
backends, harnesses, and services included in your installation. A headless
worker's terminal displays its transcript; it is not a native harness TUI.

The documentation is installed with the command and works offline:

- `agentctl quickstart` gets the first session running.
- `agentctl userguide` explains ownership, recovery, goals, and available extensions.
- `agentctl COMMAND --help` describes that command's arguments and examples.
- `agentctl chat quickstart` and `agentctl chat userguide` explain Google Chat setup
  when the Chat extension is available.

Paused or uncertain work remains visible. A delivered instruction or an idle
terminal does not establish that the agent's task is complete.
