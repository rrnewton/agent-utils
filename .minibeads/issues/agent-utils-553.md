---
title: 'voice-ws-relay-auth: serve the voice socket through vibe-talk with its own authentication'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- voice
- security
created_at: 2026-10-26T20:40:55.149723461+00:00
updated_at: 2026-10-26T20:40:55.149723461+00:00
---

# Description

[opus 5.5] A deployment-managed voice bridge (the vibe-talk-v1 WebSocket) authenticates nobody. A TLS front door that forwards /ws straight to it makes the voice agent, and its chat tools, reachable by anyone who can reach the front door, and a second vibe-talk instance that wants to use the same bridge for read-aloud has no credential to present and no way to trust a privately issued certificate.

vibe-talk should serve the voice socket itself and relay it to a bridge it alone can reach (on loopback):
- a browser page cannot set Authorization on a WebSocket, so the voice-session call it already makes (bearer-authenticated) returns a socket URL carrying a short-lived, single-use ticket;
- a server client (another vibe-talk instance doing read-aloud) presents a bearer token of a new voice-only scope, which opens the relay and nothing else, and gets no chat tools;
- on the client side, read-aloud gains websocket_token_file (sent as Authorization: Bearer) and websocket_ca_file (an extra PEM root for a privately issued certificate), used by the read-aloud session and the bridge capability probe alike.

# Acceptance Criteria

With conversation.relay_upstream set, GET /api/v1/voice-socket upgrades only with a valid unexpired single-use ticket or the voice-scope bearer token, and relays frames both ways to the upstream; anything else gets the same 401 as the rest of /api/. The page uses the relayed URL with no front-door /ws route. A voice-scope caller cannot reach any other route and cannot enable tools. Read-aloud with websocket_token_file and websocket_ca_file connects to a wss:// relay on another host. Unit and route-inventory tests cover all of it; the README and example configuration describe the setup.
