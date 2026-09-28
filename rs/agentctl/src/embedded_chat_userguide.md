# agentctl chat — durable event-driven coordinator bridge

`agentctl chat` connects an already registered interactive agent to a chat
subscription plugin. The plugin supplies normalized inbound events through the
provider-neutral `chat-subscription` traits. A separately configured one-shot
command performs replies and reactions. Neither executable obtains authority
from its manifest: the operator chooses routing, sender allowlists, environment
names, and the exact outbound helper path.

The bridge admits each provider batch and its replay cursor durably before it
acknowledges that batch. It writes a bounded `prepared` receipt first, calls the
plugin commit callback, waits for the exact matching `Committed` frame, and only
then atomically replaces that slot with a `committed` receipt containing the
delivery ID, provider sequence, host batch sequence, cursor, event count, and
timestamp. A `prepared` receipt is never reported as confirmed. This artifact
proves that the local plugin callback was released and returned exact protocol
confirmation; it does not prove a later provider network or server-side action.
The bridge then sends the request to the configured named agent.
Replies use the exact `CHAT_REPLY` fence supplied in the prompt and are retained
with one stable outbound UUID before any send attempt. A restart retries the same
operation identity; an unknown transport result is never converted into success.

## Configuration

Create a private owner-only JSON file (normally mode `0600`):

```json
{
  "subscription_plugin": "provider-events",
  "subscription_environment": ["PROVIDER_CERT", "PROVIDER_KEY"],
  "channel_ids": ["spaces/example"],
  "allowed_senders": ["users/owner"],
  "agent_name": "coordinator",
  "agent_label": "codex coordinator",
  "outbound_enabled": true,
  "ack_reaction": "🤖",
  "backend_configuration": {
    "schema": "provider.example/v1",
    "data": {}
  },
  "outbound_command": {
    "executable": "/absolute/path/to/reply-helper",
    "arguments": [],
    "environment": ["PROVIDER_ACCOUNT"],
    "timeout_millis": 30000,
    "shutdown_grace_millis": 2000
  }
}
```

The subscription plugin is discovered below `$AGENTCTL_HOME/plugins` (default
`~/.agentctl/plugins`) using its private manifest. The bridge revalidates and
pins the manifest directory and executable before every generation. Provider
credentials do not belong in the manifest or saved bridge state. When a plugin
needs credential paths or tokens from the service environment, list only their
variable names in `subscription_environment`. Names must be unique shell
identifiers, at most 128 bytes each, with no more than 64 names. Every named
value must be present before the plugin can be launched. The host clears the
plugin environment, restores its small documented non-secret baseline, then
adds exactly these operator-selected names. Values are neither serialized nor
reported by `chat status`; a plugin manifest has no authority to add names.

The outbound helper receives exactly one newline-terminated JSON request and
must emit exactly one newline-terminated JSON response. It is started inside the
same reviewed pidfd/private-process-group supervisor used for subscription
plugins, with a cleared environment plus only the configured names. The host
sets `AGENTCTL_PROCESS_SUPERVISED=1`. The helper must not create another process
group. Its operation deadline is positive and at most 30 seconds. At service
startup the host validates and hashes a native helper of at most 64 MiB, copies
those exact bytes into a sealed in-memory executable, and retains that image for
the generation. Reply and reaction operations neither reopen nor rehash the
source path.

If outbound messages use an allowed sender identity, prevent their labelled text
from becoming new requests with a runtime-only exclusion, for example
`chat run --bridge-state PATH --ignore-text-prefix '[assistant'`. Repeat the
option for up to 32 prefixes. Matching is literal and case-sensitive after
leading Unicode whitespace; each prefix must be nonempty, at most 256 UTF-8
bytes, and contain no control characters. The default excludes nothing.
Ignored messages still participate in the original provider batch fingerprint,
replay boundary, and cursor commit, but create no request, reaction ACK or pane
delivery. This does not suppress already admitted work. Supply the options on
every run; they do not change the persisted configuration or apply to `chat tick`.

In `chat run`, a generation-owned worker queues reaction ACKs as soon as the
inbound batch is durably admitted and its provider commit is confirmed. Pane
delivery, later intake, output capture and replies do not wait for that worker.
It runs one ACK at a time through a bounded queue; overflow stays in durable
state for recovery. Failed or uncertain ACKs retain their operation ID and wait
at least 60 seconds before an in-process retry. A restart reconciles pending
ACKs using those same IDs. `chat tick` still completes its bounded ACK work
before returning. During service shutdown, queued work remains pending and an
already admitted ACK is owned until bounded completion or uncertain cleanup.

To run intentionally without reactions or replies, set `outbound_enabled` to
`false`, set `ack_reaction` to `null`, and omit `outbound_command`. The delivered
prompt then explicitly identifies the bridge as inbound-only and forbids reply
fences.

## Initialize and run

The named agent must already exist in the selected registry. Every channel in
`channel_ids`, and every thread within it, routes to that one agent. To give
several agents their own conversations, give each agent its own channel, its
own config, and its own bridge state. Two bridges cannot split one shared
channel by thread. Initialization checks the live pane, plugin installation,
outbound executable, and complete configuration before it creates state:

```sh
chmod 600 chat.json
agentctl --registry /work/project/.agentctl chat init \
  --config chat.json \
  --bridge-state /home/me/.local/state/agentctl/project-chat

agentctl --registry /work/project/.agentctl chat run \
  --bridge-state /home/me/.local/state/agentctl/project-chat
```

`run` owns an exclusive state lease. Provider intake blocks on the plugin event
stream; it does not poll a REST listing. Terminal reply capture blocks on
Herdr's `events.subscribe` socket with bounded groups of current closing-fence
IDs and a generic predicate for unavailable IDs. The current groups rearm when
reply routes change, so a consumed closing fence left on screen cannot mask the
next reply. Exact IDs route through the in-memory durable-state index; all
2,048 active requests fit within the subscription budget of 128 predicates,
32 KiB per predicate and 96 KiB total. Provider notices and SIGINT/SIGTERM
interrupt that wait through
a local wake descriptor. A disk-backed terminal and delivery reconciliation
occurs every 300 seconds by default and can be changed with
`--reconcile-interval`.

A provider `Gap` is a terminal continuity warning in protocol v1. Its reason does not
contain a recoverable range or completeness proof, so the host journals the
incident, keeps the prior safe cursor, sends no commit, cancels that provider
generation, and exits degraded. `chat status` reports `healthy: false` and the
exact unresolved incident; `chat run` refuses automatic reconnect. One later
message or checkpoint does not prove that a missing interval was recovered.

An operator may approve a retry only when the saved boundary is exactly one
committed `Checkpoint`, the gap proposes that same cursor, and independent
provider evidence establishes that retrying that fixed cursor is safe. Stop the
runner and review its backend's explicit retention-boundary recovery policy
first. The host does not interpret a provider's free-text reason as proof.

```sh
agentctl chat retry-checkpoint-gap \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --expected-gap-sha256 "$GAP_SHA256" \
  --expected-checkpoint-sha256 "$CHECKPOINT_SHA256" \
  --expected-configuration-sha256 "$CONFIGURATION_SHA256" \
  --keep-cursor "$COMMITTED_CURSOR" \
  --evidence-file /home/me/private/provider-evidence.json \
  --evidence-sha256 "$EVIDENCE_SHA256"
```

Supply lowercase SHA256 digests of the exact reviewed `gap.json`,
`checkpoint.json`, `bridge.json`, and evidence file bytes. Evidence must be a
private nonempty JSON object of at most 64 KiB. It is retained as an explicit
operator attestation, not a host-verified claim about provider history. The
command verifies the stopped runner lease, all local digests, and the exact
committed checkpoint receipt; it refuses message-bearing boundaries, including
messages excluded by a runtime prefix filter.

Approval writes only a bounded audit under `gap-retries/`; it preserves the
cursor, request bytes, UUIDs, and unresolved diagnostic. Status reports
`gap_retry_approved: true` and `healthy: false`. The next generation may admit
only the identical checkpoint at the kept cursor. A new exact provider commit
resolves the gap and clears reconciliation; an interrupted resolution is
repaired from that new committed receipt. A new gap revokes the approval before
it is published. Audits retain original state and evidence without eviction,
with a limit of 64 records and 512 KiB per record. This procedure cannot reset
to the provider head, accept loss, replay quarantined work, or claim recovery
before provider confirmation.

For a bounded local work recovery pass while the daemon is stopped (`tick` does
not repair provider gaps):

```sh
agentctl --registry /work/project/.agentctl chat tick \
  --bridge-state /home/me/.local/state/agentctl/project-chat
```

Inspecting status is provider- and Herdr-free and does not perform recovery
writes:

```sh
agentctl chat status \
  --bridge-state /home/me/.local/state/agentctl/project-chat
```

Inspect one exact active retained request for latency/audit evidence without a
provider, outbound helper, Herdr call, or state write:

```sh
agentctl chat inspect \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --request 64_LOWERCASE_HEX_CHARACTERS
```

The stable output schema is `agentctl-chat-request-inspection/v1`. It contains
the request phase and source IDs; `provenance.host_batch_sequence`,
`provider_sequence`, `delivery_id`, and cursor; `timestamps.admitted_at_millis`,
`delivery_started_at_millis`, `delivered_at_millis`, `ack_started_at_millis`,
and `ack_completed_at_millis`; delivery and acknowledgement receipts; and a
bounded `replies` array with each ordinal, phase, operation ID, provider message
ID, `captured_at_millis`, and `sent_at_millis`. Start timestamps record the first
durable attempt. Delivery, ACK, and reply completion timestamps remain null
after failed or outcome-unknown operations and are written only with positive
Herdr/provider evidence. The command accepts one exact key and reports only an
active retained request. An acceptance harness that later calls `chat close`
must copy and fsync this inspection document first.

An owner or operator can explicitly publish a new root message through the
configured outbound helper without pretending it is a reply:

```sh
agentctl chat publish \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --channel-id spaces/example \
  --request-id 123e4567-e89b-42d3-a456-426614174000 \
  'Please reply to this bridge test.'
```

`publish` accepts `--file PATH` instead of positional text. It requires an
outbound-enabled state and an exact channel from `channel_ids`, validates a
nonempty body of at most 30,000 UTF-8 bytes, and sends `thread_id: null`. The
lowercase RFC 4122 version-4 UUID is caller-owned; an uncertain result may be
retried only with the identical UUID, channel, and body. The command runs the
helper only through the reviewed process supervisor, binds the returned message
resource to the requested channel, and prints the exact v1 success receipt. It
neither writes bridge state nor creates an event-loop reply route; this is an
explicit operator action, not automatic subscription behavior.

Explicitly close reply capture for an old request:

```sh
agentctl chat close \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --request 64_LOWERCASE_HEX_CHARACTERS
```

Closure is the provider-neutral terminal route lifecycle; idle/done status and
the first reply do not close a route because a request may send later progress
updates. A closed request remains active until its prompt delivery is confirmed,
its reaction is disabled or confirmed, and every captured reply is sent. It is
also retained while it belongs to the current inclusive replay cursor. Once all
conditions hold and a later cursor is durable, the bridge fsyncs a retirement
journal containing the exact reaction and reply operation/provider receipts,
installs a compact replay and closed-nonce guard, and only then deletes the
active request/reply files. Startup completes an interrupted retirement.

The active population remains capped at 2,048 requests. The compact route and
replay ring retains the latest 4,096 retired identities, matching the pane
marker bound, and the parallel retirement audit ring has 4,096 slots of at most
256 KiB each. Older replay safety relies on the subscription contract's
inclusive monotone cursor; a cursor regression or a reused message identity
with different content fails closed. This is bounded local retention, not a
claim of infinite local audit history.

A reply marker in the pane whose ID is not available, such as a typo or a stale
block left in scrollback by another bridge state, produces one routing-error
prompt to the agent. The prompt names the unavailable ID and up to 32 of the
reply IDs that were available when it was written, and counts the rest. It is
never posted to chat. Reported IDs are kept in `fence-feedback.json` in the
bridge state directory, so each unavailable ID is reported at most once per
state directory, including after a restart. A new state directory starts with no
reported IDs. A marker that stays visible after its report is left out of later
prompts, and a later block that reuses a reported ID is not reported again. An
ID reported because it was ahead of its open request's next reply ID becomes the
next reply ID once the request's earlier replies are captured, and a block under
it is then posted like any other reply, even the reported block if it is still
on screen. So if the agent answers under such an ID and then, as the prompt
asks, sends the answer again under an available ID while the first block is
still on screen, the answer can be posted twice. The request's next reply ID is
itself reported when a recovery scan sees a marker with that ID but no complete
block under it, such as an opening marker before its closing one, or a closing
marker whose opening one has scrolled away. The prompt then names that ID as
unavailable, and also as available if it is among the available IDs it lists. A
complete block under it is posted like any other reply. A block under any other
reported ID is never posted. Its only trace is a log line, and only recovery
scans write that line. Recovery scans run when `chat run` starts, at each
`chat tick`, at each reconciliation while the agent pane is idle or done, when
the pane settles idle or done, and after output names a reply ID that belongs to
no request the bridge knows. Each recovery scan that sees reported IDs that are
still unavailable logs one `already reported, so not repeated` line that names
up to 8 of them and counts the rest, up to 128 per scan. Other captures log
nothing, so a reused ID that leaves the screen before the next recovery scan
leaves no trace. Already reported markers are set aside before the bound on new
ones, so a screen full of old markers cannot hide a new one. While a
routing-error prompt is still queued, newer unavailable IDs wait for it instead
of producing a second prompt. The exact pending prompt is saved before
submission, so recovery settles its original queue ID even if a crash hides the
submission result or newer unavailable markers appear. A prompt whose queue
outcome is uncertain counts as reported: it is not submitted again, even if it
never reached the agent. The history retains up to 4,096 distinct reported or
pending IDs. At that limit, new diagnostics stay held; reported IDs are never
evicted or submitted again. Deleting `fence-feedback.json` clears the history,
so the IDs it held can be reported once more. If the file cannot be read or is
outside its bounds, the error names it and diagnostics stay held until it is
repaired or deleted.

A per-thread post-rate breaker bounds any remaining reply loop. One provider
thread may reserve 8 distinct reply operations within 60 seconds. That leaves
room for several requests in one thread, each with progress updates and a
multi-message answer, and each request prompt states this budget to the agent.
The next reply to that thread trips the breaker. Replies to that thread then
stay captured but unsent for 300 seconds; replies to other threads are
unaffected. Held replies go out in order, under their original operation IDs, at
the first retry after the cooldown ends: the next reconciliation (every 300
seconds by default; see `--reconcile-interval`), the next time the agent pane
settles idle or done, the next captured reply for the same request, a restart of
`chat run`, or `chat tick` while the daemon is stopped. Those sends count
against a new window, so a longer backlog goes out 8 at a time, with a new hold
after each group of 8. The breaker therefore bounds only fast loops. A loop
never trips if each post starts at least 60 seconds after the receipt of the
post 8 before it, so it can post up to 480 times an hour to one thread. A faster
loop trips on its 9th post within 60 seconds; one that posts every 3 seconds
trips 24 seconds in. It then sends 8 more posts after each 300-second hold,
about 96 an hour. While a thread is held, every attempt fails with a
`post-rate breaker` error that names the thread and the release time in
milliseconds after the Unix epoch. `chat run` logs that error, and `chat tick`
reports it and exits with status 75. `chat status` reports the breaker under
`reply_breaker`: its limits (`max_replies_per_thread`, `window_seconds`, and
`cooldown_seconds`), each held thread in `held_threads` with
`held_until_millis`, `held_for_seconds`, and
`recent_or_unresolved_reservations`, and an `error` that is null unless the
record is unusable. Status never changes the record, and it lists only threads
whose trip is still live: a thread whose trip has expired is not listed even if
8 of its reservations are still unresolved, and its next new reply trips the
breaker again. Neither the agent nor the chat thread is told about a hold: a
notice to the agent could prompt more replies, and a notice in the thread would
be one more post to the thread being held.

To release held threads early, first confirm that no reply loop is running, then
delete `reply-breaker.json` from the bridge state directory. This releases every
thread and discards the budget history of all of them; held replies go out at
the next retry. If that file cannot be read or is outside its bounds, replies to
every thread stay held until it is repaired or deleted: each publish error names
the file, and `chat status` shows the failure in `reply_breaker.error`. A
reservation or trip stamped later than the current time, as after the wall clock
steps back, is moved to the current time, so its window or cooldown restarts
once instead of lasting as long as the step. The next send attempt to any thread
saves the moved stamps; until then, `chat status` reports such a trip as held
for a full cooldown from the time of each call. Reservations are durable before
a provider call, including calls whose outcome is unknown. Unresolved
reservations keep their budget until the same operation is reconciled or the
ledger is explicitly reset. A valid receipt starts its 60-second retention
window; retrying an expired completed reservation must pass the current budget
again. A full ledger holds new sends until completed entries expire. This can
conservatively hold replies after a failed attempt.

## Service management

`chat run` is a foreground process and exits cleanly after SIGINT or SIGTERM.
A service manager should restart it on failure and use the same state directory.
Size task and memory limits for the selected provider implementation; those
costs are outside the provider-neutral host and can differ substantially between
plugins. Disable swap for a latency-sensitive bridge only after giving the
provider enough physical-memory headroom.

Derive the hard stop interval from the selected plugin manifest rather than
copying a universal number. Before Hello completes, the host may need
`hello_seconds + close_seconds + shutdown_grace_seconds + 7` seconds. After
Hello, it may need `close_seconds + shutdown_grace_seconds + 7`. The final seven
seconds reserve the process supervisor's two-second forced-reap bound and five
seconds for host reconciliation and worker joins. Start is interruptible after
Hello, so its independent phase timeout is not additive. An admitted reaction
or reply helper separately owns `outbound_command.timeout_millis +
outbound_command.shutdown_grace_millis + 7000` milliseconds. Set
`TimeoutStopSec` to at least the maximum of the applicable provider window and
that outbound window, plus measured service-manager scheduling margin. The
maximum accepted outbound window is 67 seconds. At startup the host pins the
complete selected provider timeout tuple. A later generation whose manifest
differs is rejected until the service restarts, so the configured outer bound
cannot silently become stale.

With systemd 258 or newer, a synchronous same-binary `ExecStop` can address only
the exact main-process identity. `graceful-stop-main` opens a pidfd for
systemd's `MAINPID`, requires its inode to equal `MAINPIDFDID`, sends SIGTERM
through that pidfd, and waits for exact process exit. It does not return at its
70-second diagnostic threshold, because returning would begin a second stop
phase. Pair it with `TimeoutStopFailureMode=kill`: at the one service-manager
deadline, systemd kills the complete control group rather than starting another
grace interval.

For example, a user service can run the same foreground command (replace every
absolute placeholder and tune the resource ceilings from a measured provider
probe):

```ini
[Unit]
Description=agentctl chat bridge
After=herdr.service

[Service]
Type=exec
Environment=AGENTCTL_HOME=/home/USER/.agentctl
ExecStart=/absolute/path/agentctl --registry /work/project/.agentctl chat run --bridge-state /home/USER/.local/state/agentctl/project-chat
ExecStop=/absolute/path/agentctl chat graceful-stop-main --main-pid ${MAINPID} --main-pidfd-id ${MAINPIDFDID}
Restart=on-failure
RestartSec=2
KillMode=control-group
OOMPolicy=kill
# Replace every value below with a measured deployment-specific value.
TimeoutStopSec=<DERIVED_SECONDS_WITH_MARGIN>
TimeoutStopFailureMode=kill
TasksMax=<MEASURED_TASKS_WITH_HEADROOM>
MemoryHigh=<MEASURED_RECLAIM_THRESHOLD>
MemoryMax=<MEASURED_HARD_LIMIT>
MemorySwapMax=0
CPUQuota=<MEASURED_CPU_LIMIT>

[Install]
WantedBy=default.target
```

After installing the unit as `agentctl-chat.service`, make persistence explicit
and verify both properties instead of merely starting an ephemeral process:

```sh
systemctl --user daemon-reload
systemctl --user enable --now agentctl-chat.service
systemctl --user is-enabled agentctl-chat.service
systemctl --user is-active agentctl-chat.service
```

`enable --now` survives a user-manager restart; operation while the user is
logged out additionally requires lingering to be enabled for that account.
`Type=exec` makes an `active` transition contingent on successful executable
startup. `KillMode=control-group` and the one derived stop deadline keep plugin
and helper descendants inside the unit's cleanup boundary. Use the same pinned
`agentctl` executable in `ExecStart` and `ExecStop`; a mutable symlink can make
the control helper disagree with the running generation.

The resource tokens in the example are intentionally invalid placeholders, not
defaults, minimums, or evidence about an unmeasured plugin. Measure the whole
service cgroup—including every plugin and helper descendant—under bounded
end-to-end load. Choose limits with explicit headroom, verify their parsed
systemd values after reload, and then prove the selected envelope still meets
the deployment's latency target. A `TasksMax` below a plugin's startup fan-out
will prevent that generation from launching; an excessively loose limit does
not provide useful containment.

The host bounds retained requests, replies, retirement records, commit receipts, per-frame data, event drains,
mailboxes, process cleanup admissions, and helper deadlines. Ordinary provider
or terminal events touch direct durable records. Full directory scans happen at
startup and explicit/periodic recovery, not in the subscription inner loop.
