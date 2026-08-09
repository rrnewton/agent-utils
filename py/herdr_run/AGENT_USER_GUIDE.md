# herdr-agent — interactive-agent messaging

`herdr-agent` is the identity-agnostic transport for an interactive agent already
running in a Herdr pane. It durably queues prompts, waits for native `idle` or
`done`, submits the complete literal text and Enter in one `pane run`, and requires
Herdr to observe the subsequent `working` state. One lock serializes overlapping
senders. Failed prompts remain in `inbox/` for another attempt or move to `failed/`
after the configured bounded retry count; neither outcome discards the prompt.

The caller is the target-resolution adapter. Give either an exact pane or a stable
session. Assert every identity fact you know; a mismatch is a refusal:

```sh
herdr-agent status --session-agent codex --session "$CODEX_SESSION_ID" \
  --agent codex --workspace deepscry --cwd /work/mtg --queue /work/mtg/ops/state/lead

herdr-agent send --session-agent codex --session "$CODEX_SESSION_ID" \
  --agent codex --workspace deepscry --cwd /work/mtg --queue /work/mtg/ops/state/lead \
  --file /work/mtg/ops/logs/poll-prompts/2026-W32/2026-08-09/060000_poll_prompt_output.md

herdr-agent read --session-agent codex --session "$CODEX_SESSION_ID" \
  --workspace deepscry --cwd /work/mtg --queue /work/mtg/ops/state/lead --lines 500
```

An exact `--pane` is useful for named-agent registries. Pair it with `--session`,
`--workspace`, and `--cwd` whenever those facts are known, so a stale pane ID cannot
silently retarget delivery.

Queue layout:

- `inbox/*.json`: prompts awaiting confirmed submission, including failure details;
- `processed/*.json`: prompts whose idle/done → working transition was confirmed;
- `failed/*.json`: poison prompts retained after bounded retries;
- `.delivery.lock`: serialization across cron ticks and interactive callers.

`status` prints the validated live identity/state and pending/failed IDs. `read`
returns recent unwrapped terminal text. `drain` retries an existing queue. A nonzero
result is loud; inspect the JSON artifact before retrying. The API never clears a
composer because Herdr cannot prove whether it contains a human's unsaved draft.

`send` and `drain` intentionally block while the FIFO head is busy, up to
`--ready-timeout` (900 seconds by default). They yield between state probes rather
than spin. If that bound expires, the prompt stays pending with an unchanged retry
count and the command exits with temporary-failure status 75; a later `drain` must
resume it. Long-running agents should set the bound above a normal turn duration,
or arrange a periodic/idle-triggered `drain` rather than assuming a 30-second wait
will eventually deliver.
