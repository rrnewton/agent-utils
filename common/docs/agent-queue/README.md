# Durable prompt queues for interactive coding agents

A program that types prompts into a coding agent's terminal must never type one
prompt twice, must never lose one, and must be able to say afterwards what
happened to each. This Rust library holds the file and process half of that
contract, so that several programs (a drainer that types, a sender that only
enqueues, a status reader) share one on-disk format:

- `queue`: the queue directory and its phases (`inbox/`, `inflight/`,
  `processed/`, `failed/`), message identifiers, enqueue with a duplicate check
  across every phase, state lookup, withdrawal, queue order and quarantine.
- `fsio`: owner-only directories and locks, crash-safe JSON writes, and renames
  that never replace a file.
- `identity`: the identity of a process generation (`boot_id`, `pid`,
  `starttime_ticks` and executable image), whether it is still alive, and
  whether it is an ancestor of the calling process.
- `registry`: where an agent registry keeps each agent's directory and
  lifecycle locks (`agents/<name>/`, `locks/<name>.lock`, and the older
  layout's `<name>/` and `.<name>.lock`), so every program reading a registry
  finds the same agents in either layout.

An item moves to `inflight/` before it is typed, so a crash can never cause a
second paste: an item found there afterwards is quarantined, not replayed.

Prompts can also expire or be cancelled. An item in the second pending
directory, `inbox.v2/`, may carry `not_after`; a tombstone in `cancel/`, written
without any lock, asks every drainer not to type an item. A drainer checks both
after it moves an item to `inflight/` and before it types, and sets such an item
aside in `cancelled/` untyped. These directories exist only once something used
them, so a drainer that knows only the first four can tell a queue it must not
touch.

Typing into a terminal, readiness checks and cross-queue typist locks belong to
the program that drains the queue. The crate documentation describes the
on-disk format in full.

Linux only: process identity reads `/proc` and uses pidfds.
