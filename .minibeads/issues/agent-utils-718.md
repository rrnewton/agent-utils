---
title: 'vt-startup-probe-restart: a rate-limited startup probe must not restart the server'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-01T05:08:35.441466389+00:00
updated_at: 2026-11-01T05:08:35.441466389+00:00
---

# Description

[opus 5.5] On every deploy, a Slack-backed server's startup channel probe refuses with
"1 of 1 configured channel(s) could not be read" because the provider rate-limits the probe's
read: the conversations.history bucket is still empty from the previous process's reads. The
process exits, systemd restarts it 5 s later, and each restart spends the rate budget again, so
it gets worse (the last deploy took two refusals, about 40 s down, with the front door closed).

Wanted:
- The server binds and serves (from the message cache) first; the probe runs in the background
  after binding.
- A provider rate limit during the probe never ends the process. The probe waits as the provider
  asks (retry_after), else with bounded exponential backoff and jitter, and retries only the
  channels that were rate-limited. Each wait is a WARN line with the remaining budget.
- A definite failure (bad token, missing channel, missing permission) still refuses at once.
- The total wait is bounded (default 5 min, configurable); when it runs out the server exits with
  a clear error.
- /healthz says "probe pending (rate limited, retry in Ns)" rather than healthy while it waits.
- Tests: rate-limited N times then readable (ready without exiting), rate-limited forever (gives
  up after the budget), auth failure (refuses at once), and the listener answering while the
  probe is still rate-limited.
