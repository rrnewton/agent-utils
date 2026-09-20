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

   To keep an agent that is already running in Herdr, adopt its exact live
   identity instead. All four assertions are required; adoption changes neither
   the pane nor its process:

   ```sh
   agentctl adopt reviewer --pane w1:p2 --workspace project \
     --cwd /work/project --harness codex
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

5. Finish with `agentctl stop reviewer`. This stops a runtime created by
   `agentctl` and archives its session state. For an adopted agent it only
   unregisters and archives the control state; the foreign pane and process keep
   running. Exiting the CLI or closing its caller does not stop the worker.

State defaults to `.agentctl` in the current directory. Use the same
`--registry /absolute/path` across callers. Workers share the working directory
you choose; use separate worktrees when their edits need isolation.

`agentctl userguide` explains lifecycle and recovery. `agentctl start --help`
and the other subcommand help pages describe every option. Worker, Chat, and
MCP extensions are installation-dependent; inspect `agentctl capabilities`
before using their commands.

To launch a coordinator directly in the current Herdr shell pane and bridge it
for the lifetime of that session, run:

```sh
agentctl chat launch --config chat.json --model gpt-6-astra
```

The reusable Chat config supplies the space, allowed senders, and transport;
`launch` supplies the current pane, workspace, and working directory. Subagents
started by that coordinator inherit the same Herdr workspace. Run
`agentctl chat quickstart` for the compact configuration and
`agentctl chat userguide` for authentication, recovery, and the separate-daemon
alternative.

The Chat bridge accepts messages from configured senders, acknowledges intake
with 🤖, and sends the agent's tagged replies back to their originating chat
thread. One request can receive multiple replies, including progress updates.
The agent normally needs no reply-file write or reply command. Thread
replies also include a command hint for reading the nearest ten prior messages.
The built-in transport polls the public Google Chat API. For prompt intake from
a persistent event stream, configure an `event_command` adapter as described in
`agentctl chat userguide`; ACKs, prompt delivery, and recovery scans then run
independently.
