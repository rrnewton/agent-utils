---
title: 'provider-diagnostics-ids: a test reaches the real Discord API, so it fails wherever that API answers; keep every test client on loopback'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T09:35:54.187373887+00:00
updated_at: 2026-10-19T09:39:48.619042149+00:00
closed_at: 2026-10-19T09:39:48.619042029+00:00
---

# Description

[opus 5.5] Follow-up to #325 provider-diagnostics-ids, which delivered the test that fails here.

`vibe-talk/tests/provider_diagnostics_ids.rs` `with_both_providers_each_check_is_named_for_its_own` builds its providers from `vibe_talk::testing::config_toml()`, whose legacy `[discord]` section has no `api_base`, so its Discord client talks to the public Discord API. Where that API answers (hosted CI), it rejects the test token with 401, the diagnostics channel probe stops after the first channel, and the Slack channel check is missing: the test has failed in every repository-ci run that reached it since #325 landed (every push to main on 2026-10-18 and 2026-10-19 that selected vibe-talk), and it passes on a host with no direct internet access, where the request fails at the transport and the probe goes on.

More tests can do the same: any test that builds a client from a configuration naming a vendor's real API base. One more builds such a Discord client today without sending a request; several build Slack or ElevenLabs clients on the public bases and send nothing.

Fix: build every vendor HTTP client (Discord protocol, Slack, ElevenLabs, the summariser's Messages API) from one builder. Once a process has used `vibe_talk::testing`, that builder resolves only `localhost`, uses no proxy, and fails any other host name at name resolution, before a packet leaves. The server never calls `testing`, so production is unchanged. Every client built after a test used `vibe_talk::testing` reads a real API base as unreachable on every host. Websocket clients (speech, the voice bridge) are not reqwest clients and are out of scope; tests point them at local mocks today.

Acceptance:
- the both-providers test passes in hosted CI, and asserts that its Discord token check reads as unreachable, not as a rejected token;
- a unit test shows a confined client reaches 127.0.0.1 and localhost and refuses a public name before any connection;
- a test shows a confined client never tunnels through an HTTP(S) proxy named in the environment (it fails before the change on any host).
