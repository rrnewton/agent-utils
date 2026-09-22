# agentctl chat quickstart

Connect one registered interactive Herdr agent to an installed event-driven
subscription plugin. The bridge keeps provider replay state, prompt-delivery
state, reply fences, and stable outbound operation IDs under one private state
directory.

1. Confirm that the provider plugin is safely discovered and that the target
   agent is live:

   ```sh
   agentctl capabilities
   agentctl --registry /work/project/.agentctl status coordinator
   ```

2. Create an owner-only `chat.json`. Replace every example authority and path:

   ```json
   {
     "subscription_plugin": "provider-events",
     "channel_ids": ["spaces/example"],
     "allowed_senders": ["users/owner"],
     "agent_name": "coordinator",
     "agent_label": "codex coordinator",
     "outbound_enabled": true,
     "ack_reaction": null,
     "backend_configuration": {
       "schema": "provider.example/v1",
       "data": {}
     },
     "outbound_command": {
       "executable": "/absolute/path/to/reply-helper",
       "arguments": [],
       "environment": [],
       "timeout_millis": 30000,
       "shutdown_grace_millis": 2000
     }
   }
   ```

   The helper is independent of the inbound plugin. Omit `outbound_command`,
   set `outbound_enabled` to `false`, and keep `ack_reaction` null for an
   intentionally inbound-only bridge.

3. Initialize once, then run the foreground service:

   ```sh
   chmod 600 chat.json
   agentctl --registry /work/project/.agentctl chat init \
     --config chat.json \
     --bridge-state /home/me/.local/state/agentctl/project-chat
   agentctl --registry /work/project/.agentctl chat run \
     --bridge-state /home/me/.local/state/agentctl/project-chat
   ```

   Stop with SIGINT or SIGTERM. A service manager may restart the same `run`
   command against the same state directory.

Use `agentctl chat status --bridge-state DIR` for a read-only durable status
snapshot. Stop `run` before `agentctl chat tick --bridge-state DIR`, which runs
one bounded recovery pass. `agentctl chat userguide` documents plugin safety,
the outbound NDJSON contract, exact local commit receipts, explicit route
closure and bounded retirement, fail-closed provider gaps, recovery, and
service-manager limits. A status with `healthy: false` and an unresolved gap is
not live success; protocol v1 intentionally refuses automatic reconnect.
