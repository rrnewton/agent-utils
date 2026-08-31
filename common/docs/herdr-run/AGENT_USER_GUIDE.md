# herdr-agent — interactive-agent messaging

`herdr-agent` is the identity-agnostic transport for an interactive agent already
running in a Herdr pane. It durably queues prompts, waits for native `idle` or
`done`, submits the complete literal text and Enter in one `pane run`, and requires
Herdr to observe the subsequent `working` state. One lock serializes overlapping
senders. Safe pre-injection failures remain in `inbox/`; ambiguous post-injection
failures move to `failed/` after one injection. Neither outcome discards the prompt.

The caller is the target-resolution adapter. Give either an exact pane or a stable
session. Assert every identity fact you know; a mismatch is a refusal:

```sh
herdr-agent status --session-agent codex --session "$CODEX_SESSION_ID" \
  --agent codex --workspace project --cwd /work/project --queue /work/project/.agent-state/lead

herdr-agent send --session-agent codex --session "$CODEX_SESSION_ID" \
  --agent codex --workspace project --cwd /work/project --queue /work/project/.agent-state/lead \
  --file /work/project/prompts/next-task.md

herdr-agent read --session-agent codex --session "$CODEX_SESSION_ID" \
  --workspace project --cwd /work/project --queue /work/project/.agent-state/lead --lines 500
```

Numeric option values use ASCII decimal syntax only. `--max-attempts` and
`--lines` must be between 1 and 1,000,000. Waits must be finite seconds no
greater than 31,536,000; `--working-timeout` is positive and
`--ready-timeout` may additionally be zero.

An exact `--pane` is useful for named-agent registries. Pair it with `--session`,
`--workspace`, and `--cwd` whenever those facts are known, so a stale pane ID cannot
silently retarget delivery. If both a pane and session are supplied, they must resolve
to the same live pane.

Queue layout:

- `inbox/*.json`: prompts awaiting confirmed submission, including failure details;
- `inflight/*.json`: prompts durably marked possibly submitted before `pane run`;
- `processed/*.json`: prompts whose idle/done → working transition was confirmed;
- `failed/*.json`: ambiguous or malformed prompts retained without resubmission;
- `.delivery.lock`: serialization across cron ticks and interactive callers.

The queue and its state directories are private to the current account. Existing
same-user directories are tightened to mode `0700`; symlinked, foreign-owned, or
otherwise unsafe queue paths and lock files are refused.

The first mutation binds `target.json` to the target authority and every supplied
identity assertion. Later drains must present that same contract. If an expected
workspace or working directory legitimately changes, start a new queue rather than
silently weakening the old queue's delivery guard.

`status` prints the validated live identity/state and pending/failed IDs. `read`
returns recent unwrapped terminal text. `drain` retries an existing queue. A nonzero
result is loud; inspect the JSON artifact before retrying. Native `idle`/`done` does
not prove that a human left no unsaved composer draft. Use this transport only with
dedicated unattended panes: it never clears a composer, and a pre-existing draft may
otherwise be combined with or submitted by the injected text and Enter.

After queue and target serialization, `send` and `drain` wait while the FIFO head is
busy, up to `--ready-timeout` (900 seconds by default). They yield between state
probes rather than spin. Lock contention is a separate wait: one active sender owns
the target until its current delivery decision is durable, so total wall time can
exceed the readiness bound. If readiness expires, the prompt stays pending with an
unchanged retry count and the command exits with temporary-failure status 75; a later
`drain` must resume it. Long-running agents should set the bound above a normal turn
duration, or arrange a periodic or idle-triggered `drain` rather than assuming a
short wait will eventually deliver.

After submission, `--working-timeout` bounds the wait for Herdr's native working event.
The control process receives a small additional shutdown margin; a missing confirmation
is ambiguous and is quarantined rather than automatically retried.

Readiness and target validation happen while the FIFO head remains in `inbox/`, so a
crash during a normal busy wait stays safely pending. Immediately after `idle` or
`done` is observed and before injection, the head is durably moved to `inflight/`;
all file and directory transitions are synced to disk. If the sender crashes after
that point, the next drain moves the inflight artifact to `failed/` as possibly
submitted without reinjection. Invalid JSON or schema is preserved byte-for-byte in
`failed/` with a separate `.error` metadata file, and later valid FIFO entries continue.

Machine outcomes are distinct: exit 75 plus JSON `outcome: pending` means nothing was
injected and the artifact is safe to retry; exit 76 plus `outcome: possibly_submitted`
means it must be inspected before any manual retry. Successful sends emit
`outcome: delivered`.

Production resolves Herdr from fixed install locations and refuses a group/world-
writable binary. Install or repair a per-user copy with
`chmod go-w "$(readlink -f ~/.local/bin/herdr)"`, then run `status` as a preflight
before enabling unattended delivery.
