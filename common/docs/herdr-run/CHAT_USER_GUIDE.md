# herdr-chat — message a native coordinator from Google Chat

`herdr-chat` connects one running Codex or Claude coordinator to one Google Chat
space. The coordinator keeps its native tools, instructions, context, and terminal
interface. It can manage long-lived workers with `herdr-agent` or
`herdr-subagents`; only the coordinator needs chat access.

Install `herdr-run`, authenticate your harness, and start a dedicated coordinator:

```sh
herdr-agent start coordinator --harness codex --cwd /work/project \
  --registry /work/project/.herdr-agents
herdr-agent status --name coordinator --registry /work/project/.herdr-agents
```

Use the returned pane and workspace identity in `chat.json`. Choose a model you
can access with the launch command's `--model` when the harness default is unsuitable.
Use `--harness claude` for a Claude coordinator.

```json
{
  "space": "spaces/YOUR_SPACE_ID",
  "allowed_senders": ["users/YOUR_GOOGLE_USER_ID"],
  "agent_label": "your-coordinator-model",
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
space and create replies, using appropriate Google Chat read and write scopes for
your user or Chat app. Add that identity to the space. Token acquisition and refresh
belong to your deployment; tokens are never saved in bridge state. For automatic
refresh, set `token_command` to an argument array such as
`["gcloud", "auth", "print-access-token"]`; it runs before every API request and
must print only the current token to stdout. Renewing an environment-only token
requires restarting the bridge with the new environment. Alternatively,
configure a command transport below to use an existing authenticated client.

```sh
herdr-chat init --config chat.json --state /work/project/.herdr-chat
herdr-chat run --state /work/project/.herdr-chat
```

Initialization starts at the current time. It does not replay the space's history.
Use an explicit RFC3339 `--after` on `init` to replay a chosen interval. `run` polls
every three seconds; `--interval` accepts 0.1–60 seconds. A service manager can
restart the process using the same state directory. `tick` performs one polling
and delivery cycle, and `status` displays each request and its current phase.
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
answer to a UTF-8 file and runs the supplied `herdr-chat reply` command. The bridge
then posts that answer in the originating thread, prefixed with `agent_label`.
This explicit reply artifact works with both harnesses and avoids treating terminal
redraws or tool output as an answer. Final replies may contain up to 30,000 UTF-8 bytes.

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

Replies use a stable UUID request ID. If the upstream acknowledgement is lost,
retries reuse that ID. A command adapter **must** honor idempotent sends. Google
Chat's own request-ID retention is finite; inspect very old uncertain replies
before restarting after a long outage. Poll cursors, allowlisted senders, target
assertions, and reply identity are bound to the initialized state; use a new state
directory when changing this authority. Run one bridge per coordinator and space.

## Existing-client command transport

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
