---
title: 'slack-crewmate-outbound: the Slack outbound helper always passes --as, which a Crewmate-run client refuses'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-10-19T21:53:26.645806646+00:00
updated_at: 2026-10-19T21:53:26.645806646+00:00
---

# Description

[muse watermelon] The Slack outbound helper always passes --as=<app id> to the Slack client. When the client runs as a Crewmate (the way a bridge that posts as a registered app actually runs), the client refuses --as because a Crewmate already acts through its recorded app. Separately, the bridge runs the client with a scrubbed environment that drops the user TLS certificate variables the client needs to mint the Crewmate credential, so the operator has to wrap the client in a script that drops --as and sets those variables. The helper should have a Crewmate mode that omits --as and passes the needed environment through, and the guide should say which environment the client needs. Filed only; not part of the agent-watch change.

# Acceptance Criteria

The outbound helper can run in a Crewmate mode that never passes --as and supplies the documented credential environment; the guide names the environment the client needs.
