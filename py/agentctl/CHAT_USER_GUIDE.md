# agentctl chat — message a native coordinator from Google Chat

`agentctl chat` lets you message an existing Codex or Claude agent **running in a
Herdr terminal** from one Google Chat space. Use it to send work or ask for
progress while away from that terminal, and receive the agent's replies in
the originating Chat thread. The agent keeps its native tools, instructions,
conversation, and terminal interface.

One agent is enough: no worker team is required. The agent can also act as a
coordinator and manage workers with `agentctl`; only that coordinator needs chat
access.

## How the connection works

The bridge accepts messages from configured senders and stores them in a durable
queue. By default it polls Google Chat's public REST API. An optional
`event_command` supplies a persistent event stream for prompt intake without
waiting for the next poll. It waits for the target's ready state, then calls
`herdr agent prompt PANE TEXT`. **Herdr submits the text through the terminal's
paste and Enter sequence.** Input does not arrive through a native harness
channel or protocol connection. Herdr must report the subsequent working state
to confirm submission.

For each response, the agent uses the provider-independent reply tags supplied
in the prompt. The bridge captures each complete block and posts it as a separate
message in the original thread. The same request can receive progress updates
followed by a final answer. The agent needs no file write or reply-command
invocation for this normal path.

`agentctl chat run` is a separate, long-running process. The Herdr server owns the
agent's terminal and harness process. The bridge does not launch or restart the
agent: stopping either process leaves the other running. The bridge owns its
local reply state. History and manual recovery commands need access to that
state and, for history, the configured Chat transport.

## Setup

This guide applies when `agentctl capabilities` lists the Chat extension.
The Python distribution includes it; no separate extra or plugin is required.
Herdr must be installed separately with its `agent prompt`, `agent wait`,
pane/session inspection, and `events.subscribe` APIs. The supported command
interface is Herdr 0.8.

Authenticate your harness and start a dedicated coordinator:

```sh
agentctl start coordinator --harness codex --cwd /work/project \
  --registry /work/project/.agentctl
agentctl status coordinator --registry /work/project/.agentctl
```

Use the returned pane and workspace identity in `chat.json`. Choose a model you
can access with the launch command's `--model` when the harness default is unsuitable.
Use `--harness claude` for a Claude coordinator.
You can instead target a suitable agent already running in Herdr: supply its exact
pane and identity assertions, and omit `agent_name` unless it has a registered
Herdr agent name. No `agentctl` manager process needs to remain running.
Herdr must observe working transitions to confirm prompt delivery. Some Claude
integrations expose session identity but leave the reported state idle during a
turn. In that case the bridge records `delivery_uncertain`; a reply artifact
still proves execution and permits the threaded response without reinjection.

### Launch from the Herdr tab you created

The shortest normal path is to create a shell tab in the desired Herdr workspace,
change to the coordinator's working directory, and run one command there:

```sh
agentctl chat launch --config chat.json --state .agentctl/chat-coordinator \
  --model gpt-6-astra
```

`launch` discovers `HERDR_PANE_ID` and `HERDR_WORKSPACE_ID`, replaces any stale
target in the reusable configuration, initializes new bridge state, starts the
bridge, and runs the native coordinator in that same pane. The bridge lives for
the coordinator's normal lifetime. Its diagnostics go to `bridge.log` under the
state directory. Exiting the coordinator stops the bridge and returns to the
shell; after killing the launcher externally, check for and stop any surviving
bridge process before reusing its state.

The launched coordinator inherits `HERDR_WORKSPACE_ID`. Consequently, its
ordinary `agentctl start NAME ...` calls create subagent tabs in the same Herdr
workspace unless it explicitly supplies `--workspace-id`.

For `launch`, `chat.json` is reusable and does not need a `target` or
`agent_name`; those fields are replaced from the current pane. A compact public
REST configuration is:

```json
{
  "space": "spaces/YOUR_SPACE_ID",
  "allowed_senders": ["users/YOUR_GOOGLE_USER_ID"],
  "agent_label": "codex-coordinator",
  "ack_reaction": "🤖"
}
```

Transport settings such as `token_command`, `transport_socket`, and
`event_command` belong in the same file. `--model` becomes the reply label by
default; use `--agent-label` when the visible label should differ. `--resume`
resumes a native conversation, and repeated `--harness-arg` values pass literal
extra arguments to the harness.

Run `launch` only from an idle Herdr shell pane. It refuses a non-Herdr terminal,
a mismatched workspace environment, a pane already hosting an agent, a missing
harness executable, or an already initialized state directory. Use one bridge
per Chat space: stop the old coordinator before launching a replacement against
the same space, and give the replacement a new state directory.

The explicit two-process setup below remains useful when a service manager owns
the bridge independently of the coordinator.

```json
{
  "space": "spaces/YOUR_SPACE_ID",
  "allowed_senders": ["users/YOUR_GOOGLE_USER_ID"],
  "agent_label": "your-coordinator-model",
  "ack_reaction": "🤖",
  "reply_mode": "tagged",
  "agent_name": "coordinator",
  "target": {
    "pane_id": "RETURNED_PANE_ID",
    "expected_agent": "codex",
    "expected_workspace": "YOUR_WORKSPACE_LABEL",
    "expected_cwd": "/work/project"
  }
}
```

`agent_name` binds delivery to the live named Herdr coordinator. Its pane is
checked before polling, at every readiness probe, and again before submission.
Delivery stops if the name no longer owns the expected pane. Give a replacement
coordinator a new name and bridge state instead of reusing that identity.

Add `session_agent` and `session_value` when the harness reports a stable session.
These are identity assertions, so a mismatch stops delivery. Keep the pane
dedicated to the coordinator and leave its input composer empty between turns.

For the built-in public Google Chat REST transport, provide an OAuth access token
in `HERDR_CHAT_TOKEN`. The token needs permission to list messages in the selected
space, create replies, and create reactions. The built-in transport uses user
authentication. A suitable scope combination is `chat.messages.readonly`,
`chat.messages.create`, and `chat.messages.reactions.create` (each under
`https://www.googleapis.com/auth/`). The broader `chat.messages` scope also
covers these operations. Add that user to the space. Token acquisition and refresh
belong to your deployment; tokens are never saved in bridge state. For automatic
refresh, set `token_command` to an argument array such as
`["gcloud", "auth", "print-access-token"]`; it runs before every transport operation and
must print only the current token to stdout. `token_env` selects another
environment variable when needed. Renewing an environment-only token
requires restarting the bridge with the new environment. Alternatively,
configure a command or socket transport below to use an existing authenticated
client.

```sh
agentctl chat init --config chat.json --state /work/project/.agentctl/.chat
agentctl chat run --state /work/project/.agentctl/.chat
```

Initialization sets the message history cutoff to the current time. Messages
created before that cutoff are excluded from both events and polls. Use an
explicit RFC3339 `--after` on `init` to replay a chosen interval. Without `event_command`,
`run` waits three seconds **after each completed polling cycle** before starting
another; `--interval` accepts 0.1–60 seconds. A cycle includes transport and
delivery work, so this is not a fixed message latency. Choose a longer interval
when your authenticated client has a shared read quota. Failures double the
delay up to 60 seconds; successful cycles restore the configured interval.

With `event_command`, incoming events wake the bridge immediately. A background
recovery scan runs on connection and every 300 seconds after a completed scan.
`--reconcile-interval` accepts 10–86400 seconds; `--interval` does not apply in
this mode. A reported event gap also requests a recovery scan. ACK requests,
Herdr prompt delivery, replies, and recovery scans run independently:
a busy coordinator does not delay an intake ACK, and a slow ACK does not delay
prompt delivery. Provider and harness latency still determine when those
operations finish. See the event adapter contract below to enable this mode.

Reply capture waits on Herdr subscriptions in both modes. A service manager can
restart the process using the same state directory. `status` displays requests,
delivery phases, ACK retry state, capture errors, and observer state. Only one
`run` or `tick` process may own a state directory: `.run.lock` enforces this.
Stop `run` before using `tick` for one polling, capture, and delivery cycle.

Messages are accepted only from the configured sender resource IDs. A distinct
bot identity for replies makes authorship clearer. Durable reply IDs suppress the
bridge's own messages even when an adapter posts as an allowlisted user. Streaming
intake defers a possible reply echo until its pending send is reconciled. A command
transport must preserve Google sender IDs rather than replace them with display
names. The bridge does not authorize messages from everyone in a space.

## Reply capture

`reply_mode` defaults to `"tagged"` for newly ingested requests. Each prompt
supplies a unique pair of provider-independent tags. The agent may reply once or
multiple times, including progress updates when requested. Each block becomes a
separate chat message. Put each tag on its own line, without an enclosing code
fence, and reuse the request's ID for every reply to that request:

```text
<CHAT_REPLY_NONCE>
The implementation is ready. I am running the checks.
</CHAT_REPLY_NONCE>

<CHAT_REPLY_NONCE>
The checks passed. The change is ready.
</CHAT_REPLY_NONCE>
```

`NONCE` stands for the unique value in that request's actual tags; do not reuse
this example literally. New requests remain available for further replies after
earlier replies are sent; a closing tag ends one message, not the request. A later
user request does not invalidate those earlier reply IDs. `GCHAT_REPLY` tags
remain accepted for prompts already in flight, while new prompts use
`CHAT_REPLY`. Completed requests saved under the single-reply protocol are not
reopened.

The daemon subscribes to reply tags through Herdr's `events.subscribe` API,
including unavailable IDs. It rearms its output observation every second so later milestones can be captured
while the agent is still working. Herdr checks text subscriptions internally
every 100 milliseconds; this is separate from the chat provider's polling
interval. Retained terminal history is bounded, so capture is not a lossless
stream of agent messages.

The bridge saves each complete tagged block durably before sending it with its
own stable reply ID. It prefixes each posted message with `agent_label`. Each
reply may contain up to 30,000 UTF-8 bytes. Text outside the tags is not sent.
Two complete identical blocks in retained output are two messages; rereading
the same snapshot does not send them again. If the original identical block has
already disappeared from retained history, a new identical block cannot reliably
be distinguished from a redraw and is conservatively treated as a replay. Use
distinct milestone text when sending repeated updates.

If the agent uses an unavailable reply ID, the bridge queues a protocol error
directly to that agent. The error identifies the unavailable ID and lists the
available request IDs and request-key prefixes so the agent can correct its
routing. The error is not posted to chat. Repeated observations of the same bad
ID and available destinations produce only one error, including after a bridge
restart. Prompt echoes and quoted code examples do not trigger these errors.

Set `reply_mode` to `"file"` to retain explicit file submission. Requests saved
before tagged capture was enabled retain their original file-reply instructions.
`agentctl chat reply` remains available for deliberate recovery in either mode:

```sh
agentctl chat reply --state /work/project/.agentctl/.chat \
  --request REQUEST_KEY --file answer.txt
```

You may run `reply` while the daemon is running. The streaming runner checks for
saved file replies at least once per second; the polling runner picks them up
on its next cycle. This pickup interval does not include the provider's send time.
File submission remains a recovery path for one immutable first reply: retrying
the same text is idempotent, and replacing it with different text is refused.
Use tagged output for subsequent replies.

## Thread context

An actual thread reply includes a prompt hint for reading the nearest ten prior
messages. A top-level message does not. The hint uses:

```sh
agentctl chat context --state /work/project/.agentctl/.chat \
  --request REQUEST_KEY_OR_UNIQUE_HEX_PREFIX --limit 10
```

This read-only command needs no live Herdr pane. It uses the saved request's
exact thread and creation time, excludes that request and later messages, and
displays one page in chronological order. It returns source/thread metadata and
an opaque `cursor` when older context is available. Read further back by
repeating the command with the same limit and `--cursor TOKEN`. The default
limit is 10; permitted limits are 1–200.

Context includes all participants in that thread as reference material. It
does not authorize their messages as new tasks. History is fetched only when
the command is run; receiving a message does not automatically load its thread.

## Reaction acknowledgements

The bridge durably saves an authorized input before reacting to its source
message. The default reaction is 🤖. Set `ack_reaction` to a Unicode emoji such
as `"😎"`, or to null or an empty string to disable ACKs. The reaction acknowledges
intake, not completion or guaranteed harness acceptance. Busy coordinators still
receive ACKs while their prompts remain queued. In streaming mode, ACKs and
prompt submissions run independently; neither waits for the other to finish.

Each request saves `ack.state` (`pending`, `acked`, or `disabled`), the configured
emoji, a stable request UUID, attempts, last attempt time, next retry time,
error, and confirmed reaction resource. Reaction failures do not abort prompt
or reply processing. They remain visible in `status` and retry after
3, 6, 12, 24, 48, then 60 seconds while the bridge is running. Restarting preserves
that state and retry identity. Unfinished requests from state created without
ACK support acquire ACK state; completed requests are not decorated retrospectively.

Google's public [reactions.create API](https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces.messages.reactions/create)
has no `requestId` argument. Optionally configure
`reaction_user` as the canonical `users/ID` of the OAuth user. The adapter then
lists that user's matching emoji before creating a reaction, allowing a lost
create response to be reconciled on retry. This needs a reaction-read scope;
`chat.messages.readonly` already permits that read. The OAuth actor can differ
from the allowed sender. `users/me` is not a supported configuration alias.
Without `reaction_user`, creation is attempted directly and unconfirmed failures
stay pending; the bridge does not claim exactly-once reaction delivery or count
somebody else's emoji as its own ACK. Keep the OAuth actor stable across retries.

## Durable state and recovery

State directories are private to the current account. `requests/` holds source
messages and delivery phases, `queue/` is the existing durable Herdr inbox, and
`replies/` holds durable reply artifacts. `feedback/` records deduplicated
protocol errors queued to the coordinator. Immutable message resource names deduplicate both
events and polls; replaying an event does not create a second prompt.
`input.json` records the stream's durable cursor and connection/recovery state.
The cursor advances only after its message has been saved or excluded by the
configured authority. A reconnect passes that saved cursor to the event adapter.
Source creation timestamps order pending delivery. REST pagination is checkpointed after source
messages are persisted, with a 60-second overlap for equal timestamps and brief
indexing delays. Long upstream indexing delays may require an explicit replay with
a new bridge state directory after checking what was already delivered.

Busy coordinators keep messages queued while intake continues. A crash or lost
working-state acknowledgement after terminal injection marks `delivery_uncertain`;
the bridge does not automatically send that prompt again. A keyed reply
from the coordinator proves execution and allows reply delivery while preserving
the failed queue artifact. Otherwise inspect the pane and
queue artifact before deciding whether a new request is needed. Provider failures
and unanswered prompts remain visible in `status`; delivery is not proof of task
completion. A blocked harness may need interaction in its pane.

After a restart or subscription reconnect, the bridge can recover a complete
tagged block only while it remains in Herdr's retained terminal history. This
history is bounded and is not a lossless output log. A visible closing tag with a
clipped or malformed block leaves a `capture_error` instead of posting a guessed
answer. If all tags have been evicted, the request remains awaiting a reply. Inspect
the pane and `agentctl chat status`. An idle/done event triggers one fresh read
to recover an early closing tag inside a quoted example; readiness itself is
never treated as an answer. Output observation stays active after capture errors
and rearms every second to detect further replies. After stopping `run`, an explicit
`agentctl chat tick` also retries capture once after inspection. Captures request
up to 4,000 retained logical lines
and are bounded to 2 MiB. If the answer cannot be recovered from retained
history, submit the verified answer with `agentctl chat reply`.

Each captured reply uses its own stable UUID request ID. If the upstream
acknowledgement is lost, retries reuse that ID before later replies for the same
request are sent. An identical explicit `reply` retry succeeds even after its
answer was posted. A command adapter **must** honor idempotent sends. Google
Chat's own request-ID retention is finite; inspect very old uncertain replies
before restarting after a long outage. Poll cursors, allowlisted senders, target
assertions, and reply identity are bound to the initialized state; use a new state
directory when changing this authority. Run one bridge per coordinator and space.

## Streaming event adapter

Set optional `event_command` in `chat.json` before initialization to receive
messages from an operator-supplied event adapter:

```json
{"event_command": ["/absolute/path/to/chat-events-adapter"]}
```

This is a separate input connection. Keep either the built-in REST transport,
`transport_command`, or `transport_socket` for reactions, replies, history,
and recovery scans.
The package does not supply a Google Workspace Events subscription or manage its
credentials. One public implementation can use
[Google Workspace Events for Chat](https://developers.google.com/workspace/events/guides/events-chat)
with Google Cloud Pub/Sub, then normalize those notifications to the protocol
below. The adapter owns subscription creation, renewal, upstream acknowledgements,
and any provider-specific reconnect rules.

The bridge launches the literal argument array without a shell. It writes one
JSON object followed by a newline to stdin, then closes stdin:

```json
{"action":"subscribe","space":"spaces/SPACE","cursor":null}
```

`cursor` is null on the first connection, otherwise the last durably processed
cursor string. The adapter must resume from that position or explicitly report
a gap. Replaying the boundary event inclusively is safe. On a first connection,
the adapter chooses its initial stream position. The bridge excludes messages
created before the initialization cutoff, including an explicit `init --after`.
The REST recovery scan fills available history from that cutoff even when the
stream begins at its current position.

Write one UTF-8 JSON object per line to stdout and flush each event promptly:

```json
{"type":"message","message":{"id":"spaces/SPACE/messages/MESSAGE","text":"Please inspect the build","sender":"users/OWNER","thread":"spaces/SPACE/threads/THREAD","created_at":"2026-01-01T00:01:00Z","thread_reply":true},"cursor":"POSITION"}
{"type":"heartbeat"}
{"type":"checkpoint","cursor":"NEXT_POSITION"}
{"type":"gap","reason":"The upstream replay window expired"}
```

`message` has the same normalized fields and identity requirements as a poll
result below. Preserve the authenticated sender ID and exact configured space;
the bridge applies the same sender allowlist to both paths. `thread_reply` must
come from provider metadata. An optional `cursor` on any event must be a nonempty
string of at most 8192 characters. A `checkpoint` advances past upstream events
that do not produce a message. A `heartbeat` reports a live connection; it does
not prove that no messages were missed. A `gap` requests a background recovery
scan. Preserve event order and emit preceding inputs before advancing a checkpoint.

Each line is limited to 1 MiB. Invalid UTF-8, duplicate JSON keys, nonfinite
numbers, or incomplete frames are errors. Stdout must contain only protocol
records; stderr is drained and discarded. The adapter must keep running until
stopped. Handle SIGTERM to unsubscribe and stop owned children: the runner allows
up to two seconds for cleanup before killing the remaining process group.
EOF or a framing failure closes its process group and triggers a new
subscription using the saved cursor, with retry delays increasing up to
60 seconds. Ordinary idle waits do not reconnect. Provider retention and REST
history availability still bound recovery; a durable cursor is not an unlimited
upstream event log.

## Existing-client transports

This adapter replaces Google Chat access, including authentication and polling.
It does not replace agent delivery: the coordinator must still run in Herdr.
Headless workers and interactive tmux sessions are not supported chat targets.

Set `transport_command` to an argument array in `chat.json`, for example:

```json
{"transport_command": ["/absolute/path/to/chat-adapter"]}
```

The bridge launches the command with literal arguments, sends one JSON object on
stdin, and expects one JSON object on stdout. It never invokes a shell. Each call
must finish within 60 seconds; diagnostics belong on stderr. The adapter owns its
credentials and stays separate from the reusable library.

For an already-running local adapter, set `transport_socket` instead of
`transport_command`:

```json
{"transport_socket": "/absolute/private/directory/chat-adapter.sock"}
```

These fields are mutually exclusive. The socket path must be absolute, and both
the Unix socket and its parent directory must belong to the current account.
The directory must have no group or other access (mode `0700`); where supported,
the connection also checks the peer's account. Start and supervise the adapter
separately; the bridge does not launch its server.

Each operation opens a connection, sends one UTF-8 JSON request followed by a
newline, and reads one JSON response followed by a newline. Successful payloads
are identical to the command transport below; a response containing `error` is
a failed, unconfirmed operation. Socket operations have a 60-second I/O timeout,
with requests bounded to 1 MiB and responses to 8 MiB. Serve connections
concurrently to let ACKs, replies, and scans proceed independently.
Herdr prompt delivery uses its separate connection. This avoids launching a
client process for each request; the adapter can retain its own authenticated
connections.

Poll request and response:

```json
{"action":"poll","space":"spaces/SPACE","after":"2026-01-01T00:00:00Z","cursor":null}
```

```json
{"messages":[{"id":"spaces/SPACE/messages/MESSAGE","text":"Please inspect the build","sender":"users/OWNER","thread":"spaces/SPACE/threads/THREAD","created_at":"2026-01-01T00:01:00Z","thread_reply":true}],"cursor":null}
```

Return messages in ascending creation order. A nonempty cursor continues the same
query; `null` means the page is complete. Sender and thread identity must come from
the authenticated upstream response. Empty/non-text messages may be omitted.
Optional `thread_reply` must be a boolean from the provider's reply metadata,
such as Google Chat's `threadReply`. Only `true` enables the thread-context
prompt hint. A thread resource alone does not establish that a message is a
reply; do not derive this flag or the thread ID from a message ID.

Context request and response:

```json
{"action":"context","space":"spaces/SPACE","thread":"spaces/SPACE/threads/THREAD","before":"2026-01-01T00:01:00.123456789Z","limit":10,"cursor":null}
```

```json
{"messages":[{"id":"spaces/SPACE/messages/EARLIER","text":"The relevant background","sender":"users/PARTICIPANT","thread":"spaces/SPACE/threads/THREAD","created_at":"2026-01-01T00:00:00Z","thread_reply":false}],"cursor":null}
```

Return at most `limit` messages from that exact thread strictly before the
cutoff, in descending creation order. Preserve fractional timestamp precision.
The CLI reverses each page for chronological display. A cursor continues the
same thread, cutoff, limit, and descending order; `null` means no older page.
For the public API, use `createTime < "TIMESTAMP" AND thread.name = THREAD`
with `orderBy=createTime DESC`. Report unsupported history or provider failures
as errors, rather than an empty successful page. Existing poll/send/react-only
adapters continue to deliver messages and replies, but need this action for
`chat context`.

Reaction request and response:

```json
{"action":"react","space":"spaces/SPACE","message":"spaces/SPACE/messages/MESSAGE","emoji":"🤖","request_id":"f17fb68a-5597-49a9-a1ab-d14b26331b0e"}
```

```json
{"id":"spaces/SPACE/messages/MESSAGE/reactions/REACTION"}
```

The adapter must ensure its reaction is present and return that reaction's full
resource name. Repeating the request must not toggle the reaction off. The stable
UUID is separate from every reply UUID and lets an adapter reconcile its own
lost responses. Report failures, including missing reaction permissions, instead
of acknowledging an operation that was not confirmed. Set `ack_reaction` to null
for a transport that does not implement reactions.

Send request and response:

```json
{"action":"send","space":"spaces/SPACE","thread":"spaces/SPACE/threads/THREAD","text":"[model] Finished","request_id":"f17fb68a-5597-49a9-a1ab-d14b26331b0e"}
```

```json
{"id":"spaces/SPACE/messages/REPLY"}
```

Repeated sends with the same request ID must return the same message resource.
Do not silently fall back to a new thread or another space. A transport adapter can
wrap an organizational Chat client without putting private endpoints or credentials
in this package. The public REST adapter uses `https://chat.googleapis.com/v1/`.

## Scope

Each bridge connects one configured space to one coordinator. It handles text
messages and explicit replies, including progress updates. It does not discover trigger keywords in
other spaces, forward attachments, or supervise the coordinator's lifetime.
Keep the input composer empty between automated
deliveries; direct human input and bridge input need coordination.
