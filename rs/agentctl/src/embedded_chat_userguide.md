# agentctl chat — durable event-driven coordinator bridge

`agentctl chat` connects an already registered interactive agent to a chat
subscription plugin. The plugin supplies normalized inbound events through the
provider-neutral `chat-subscription` traits. A separately configured one-shot
command performs replies and reactions. Neither executable obtains authority
from its manifest: the operator chooses routing, sender allowlists, environment
names, and the exact outbound helper path.

The bridge admits each provider batch and its replay cursor durably before it
acknowledges that batch. It then sends the request to the configured named agent.
Replies use the exact `CHAT_REPLY` fence supplied in the prompt and are retained
with one stable outbound UUID before any send attempt. A restart retries the same
operation identity; an unknown transport result is never converted into success.

## Configuration

Create a private owner-only JSON file (normally mode `0600`):

```json
{
  "subscription_plugin": "provider-events",
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
credentials do not belong in the manifest or saved bridge state.

The outbound helper receives exactly one newline-terminated JSON request and
must emit exactly one newline-terminated JSON response. It is started inside the
same reviewed pidfd/private-process-group supervisor used for subscription
plugins, with a cleared environment plus only the configured names. The host
sets `AGENTCTL_PROCESS_SUPERVISED=1`. The helper must not create another process
group. Its operation deadline is positive and at most 30 seconds.

To run intentionally without reactions or replies, set `outbound_enabled` to
`false`, set `ack_reaction` to `null`, and omit `outbound_command`. The delivered
prompt then explicitly identifies the bridge as inbound-only and forbids reply
fences.

## Initialize and run

The named agent must already exist in the selected registry. Initialization
checks the live pane, plugin installation, outbound executable, and complete
configuration before it creates state:

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
Herdr's `events.subscribe` socket with one exact line predicate per active reply
ID. Provider notices and SIGINT/SIGTERM interrupt that wait through a local wake
descriptor. A disk-backed terminal and delivery reconciliation occurs every 300
seconds by default and can be changed with `--reconcile-interval`.

For a bounded manual recovery while the daemon is stopped:

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

Close reply capture for an old request while retaining its history:

```sh
agentctl chat close \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --request 64_LOWERCASE_HEX_CHARACTERS
```

## Service management

`chat run` is a foreground process and exits cleanly after SIGINT or SIGTERM.
A service manager should restart it on failure and use the same state directory.
Size task and memory limits for the selected provider implementation; those
costs are outside the provider-neutral host and can differ substantially between
plugins. Disable swap for a latency-sensitive bridge only after giving the
provider enough physical-memory headroom.

For example, a user service can run the same foreground command (replace every
absolute placeholder and tune the resource ceilings from a measured provider
probe):

```ini
[Unit]
Description=agentctl chat bridge
After=herdr.service

[Service]
Type=simple
Environment=AGENTCTL_HOME=/home/USER/.agentctl
ExecStart=/absolute/path/agentctl --registry /work/project/.agentctl chat run --bridge-state /home/USER/.local/state/agentctl/project-chat
Restart=on-failure
RestartSec=2
TasksMax=2048
MemoryMax=4G
MemorySwapMax=0

[Install]
WantedBy=default.target
```

The example ceilings are not minimum requirements or evidence about an
unmeasured plugin. A provider that needs more than 2,048 tasks or 4 GiB must be
given a measured larger bound; a smaller provider should use a smaller one.

The host bounds retained requests, replies, per-frame data, event drains,
mailboxes, process cleanup admissions, and helper deadlines. Ordinary provider
or terminal events touch direct durable records. Full directory scans happen at
startup and explicit/periodic recovery, not in the subscription inner loop.
