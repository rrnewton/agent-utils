# agentctl quickstart

Use one named-session interface to delegate work to persistent coding agents
while keeping their terminals available for direct inspection.

1. Install Herdr and an authenticated Codex or Claude harness. Start a Herdr
   workspace for the project. Check this installation's adapters:

   ```sh
   agentctl capabilities
   ```

2. Start a worker in your project. Choose an accessible model with `--model` if
   the harness default is unsuitable:

   ```sh
   agentctl start reviewer --harness codex --cwd . \
     --brief 'Review the current changes and report concrete problems'
   agentctl list
   agentctl status reviewer
   ```

3. Send follow-up work and inspect progress:

   ```sh
   agentctl send reviewer 'Check cancellation and restart behavior too'
   agentctl read reviewer --lines 100
   agentctl goal reviewer
   ```

   Busy input can wait. Use `--ready-timeout 0` to leave it durably queued, then
   `agentctl drain reviewer` to attempt delivery. A native goal is authoritative
   only when the session is bound and its harness supports goal inspection.

4. To type directly, pause automated input first. Let any current turn finish,
   open the terminal, and clear any unfinished draft before resuming:

   ```sh
   agentctl pause reviewer
   agentctl attach reviewer
   agentctl resume reviewer
   ```

5. Finish with `agentctl stop reviewer`. This stops the owned runtime and
   archives its session state. Exiting the CLI or closing its caller does not
   stop the worker.

State defaults to `.agentctl` in the current directory. Use the same
`--registry /absolute/path` across callers. Workers share the working directory
you choose; use separate worktrees when their edits need isolation.

`agentctl userguide` explains lifecycle and recovery. `agentctl start --help`
and the other subcommand help pages describe every option. Worker, Chat, and
MCP extensions are installation-dependent; inspect `agentctl capabilities`
before using their commands.

To message one of these agents remotely, run `agentctl chat quickstart`.
The Chat bridge accepts messages from configured senders, acknowledges intake
with 🤖, and sends the agent's tagged final answer back to its Google Chat
thread. The agent normally needs no reply-file write or reply command. Thread
replies also include a command hint for reading the nearest ten prior messages.
