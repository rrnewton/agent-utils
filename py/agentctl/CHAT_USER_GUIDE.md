# agentctl chat — message a native coordinator from Google Chat

`agentctl chat` lets you message an existing Codex or Claude agent **running in a
Herdr terminal** from one Google Chat space. Use it to send work or ask for
progress while away from that terminal, and receive the agent's final answer in
the originating Chat thread. The agent keeps its native tools, instructions,
conversation, and terminal interface.

One agent is enough: no worker team is required. The agent can also act as a
coordinator and manage workers with `agentctl`; only that coordinator needs chat
access.

## How the connection works

The bridge polls Google Chat, accepts messages from configured senders, and
stores them in a durable queue. It waits for the target's ready state, then calls
`herdr agent prompt PANE TEXT`. **Herdr submits the text through the terminal's
paste and Enter sequence.** Input does not arrive through a native harness
channel or protocol connection. Herdr must report the subsequent working state
to confirm submission.

For the response, the agent writes an explicit answer file and runs a supplied
reply command. The bridge posts that answer in the original thread. It does not
scrape the screen for a final answer or mirror every terminal event.

`agentctl chat run` is a separate, long-running process. The Herdr server owns the
agent's terminal and harness process. The bridge does not launch or restart the
agent: stopping either process leaves the other running. The bridge and agent
must share the local reply-state filesystem and have access to the reply command.

## Setup

This guide applies when `agentctl capabilities` lists the Chat extension.
The Python distribution includes it; no separate extra or plugin is required.
Herdr must be installed separately with its `agent prompt`, `agent wait`, and
pane/session inspection APIs. The supported command interface is Herdr 0.8.

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
turn. In that case the bridge records `delivery_uncertain`; a final reply artifact
still proves execution and permits the threaded response without reinjection.

```json
{
  "space": "spaces/YOUR_SPACE_ID",
  "allowed_senders": ["users/YOUR_GOOGLE_USER_ID"],
  "agent_label": "your-coordinator-model",
  "ack_reaction": "🤖",
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
configure a command transport below to use an existing authenticated client.

```sh
agentctl chat init --config chat.json --state /work/project/.agentctl/.chat
agentctl chat run --state /work/project/.agentctl/.chat
```

Initialization starts at the current time. It does not replay the space's history.
Use an explicit RFC3339 `--after` on `init` to replay a chosen interval. `run` polls
every three seconds; `--interval` accepts 0.1–60 seconds. A service manager can
restart the process using the same state directory. `tick` performs one polling
and delivery cycle, and `status` displays each request, delivery phase, and ACK
retry state.
Choose a longer interval when your authenticated client has a shared read quota.
Failures double the retry delay up to 60 seconds; successful cycles restore the
configured interval.

Messages are accepted only from the configured sender resource IDs. A distinct
bot identity for replies makes authorship clearer. Durable reply IDs suppress the
bridge's own messages even when an adapter posts as an allowlisted user; uncertain
send acknowledgements are recovered before polling resumes. A command
transport must preserve Google sender IDs rather than replace them with display
names. The bridge does not authorize messages from everyone in a space.

The coordinator receives a reply command with each request. It writes its final
answer to a UTF-8 file and runs the supplied `agentctl chat reply` command. The bridge
then posts that answer in the originating thread, prefixed with `agent_label`.
This explicit reply artifact works with both harnesses and avoids treating terminal
redraws or tool output as an answer. Final replies may contain up to 30,000 UTF-8 bytes.

## Reaction acknowledgements

The bridge durably saves an authorized input before reacting to its source
message. The default reaction is 🤖. Set `ack_reaction` to a Unicode emoji such
as `"😎"`, or to null or an empty string to disable ACKs. The reaction acknowledges
intake, not completion or guaranteed harness acceptance. Busy coordinators still
receive ACKs while their prompts remain queued.

Each request saves `ack.state` (`pending`, `acked`, or `disabled`), the configured
emoji, a stable request UUID, attempts, last attempt time, next retry time,
error, and confirmed reaction resource. Reaction failures do not abort prompt
or final-reply processing. They remain visible in `status` and retry after
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
`replies/` holds final answers. Immutable message resource names deduplicate polls;
creation timestamps order pending delivery. Pagination is checkpointed after source
messages are persisted, with a 60-second overlap for equal timestamps and brief
indexing delays. Long upstream indexing delays may require an explicit replay with
a new bridge state directory after checking what was already delivered.

Busy coordinators keep messages queued while polling continues. A crash or lost
working-state acknowledgement after terminal injection marks `delivery_uncertain`;
the bridge does not automatically send that prompt again. A keyed final reply
from the coordinator proves execution and allows reply delivery while preserving
the failed queue artifact. Otherwise inspect the pane and
queue artifact before deciding whether a new request is needed. Provider failures
and unanswered prompts remain visible in `status`; delivery is not proof of task
completion. A blocked harness may need interaction in its pane.

An identical `reply` retry succeeds even after its final answer was posted;
a different answer for the same request is refused. Replies use a stable UUID
request ID. If the upstream acknowledgement is lost,
retries reuse that ID. A command adapter **must** honor idempotent sends. Google
Chat's own request-ID retention is finite; inspect very old uncertain replies
before restarting after a long outage. Poll cursors, allowlisted senders, target
assertions, and reply identity are bound to the initialized state; use a new state
directory when changing this authority. Run one bridge per coordinator and space.

## Existing-client command transport

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

Poll request and response:

```json
{"action":"poll","space":"spaces/SPACE","after":"2026-01-01T00:00:00Z","cursor":null}
```

```json
{"messages":[{"id":"spaces/SPACE/messages/MESSAGE","text":"Please inspect the build","sender":"users/OWNER","thread":"spaces/SPACE/threads/THREAD","created_at":"2026-01-01T00:01:00Z"}],"cursor":null}
```

Return messages in ascending creation order. A nonempty cursor continues the same
query; `null` means the page is complete. Sender and thread identity must come from
the authenticated upstream response. Empty/non-text messages may be omitted.

Reaction request and response:

```json
{"action":"react","space":"spaces/SPACE","message":"spaces/SPACE/messages/MESSAGE","emoji":"🤖","request_id":"f17fb68a-5597-49a9-a1ab-d14b26331b0e"}
```

```json
{"id":"spaces/SPACE/messages/MESSAGE/reactions/REACTION"}
```

The adapter must ensure its reaction is present and return that reaction's full
resource name. Repeating the request must not toggle the reaction off. The stable
UUID is separate from the final reply UUID and lets an adapter reconcile its own
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
messages and explicit final replies. It does not discover trigger keywords in
other spaces, forward attachments, or supervise the coordinator's lifetime.
Keep the input composer empty between automated
deliveries; direct human input and bridge input need coordination.
