# claude-stream-session

Persistent Claude Code stream-json sessions for the tools in this repository.

The problem: starting `claude -p` fresh for every small job pays full process
startup each time (measured at 11.8-14.2 s cold), while a session kept warm
answers later turns in 1.2-2.1 s. This crate holds the shared transport so
every caller gets the warm behaviour from one implementation:

- `protocol`: the stateless stream-json framing — protocol arguments,
  permission-prompt argument handling, reserved-argument validation, UUIDs,
  and stdin lines.
- `session`: one persistent `claude -p --input-format stream-json` process
  driven turn by turn, with token accounting and restart on death. Sessions
  always start with an empty tool list.

Callers build pools and recycling policy (turns, tokens) on top of
`session::Session`; the crate itself only transports.
