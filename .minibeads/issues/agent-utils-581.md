---
title: 'bridge-git-timeout-fatal: a slow git check-ignore must never stop chat run'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-10-27T15:53:00.281682659+00:00
updated_at: 2026-10-27T15:53:00.281682659+00:00
---

# Description

[opus 5.5] Another harness's chat bridge (agentctl chat run, built from main at 4c92ccba) exited with status 1 right after logging "agentctl: git check-ignore timed out after 5 seconds", after running for 1 d 9 h. Its systemd unit had no Restart= policy, so the owner's messages stopped flowing. The registry lives inside a large git repository on a heavily loaded host.

Cause: every agent operation the owner loop makes reads the registry's workspace policy, which runs git check-ignore on .agentctl/profiles.json with a 5 s timeout. A timeout returns an error, the owner loop propagates it with ?, and chat run exits.

Want:
1. A transient timeout never kills chat run: retry with backoff, keep the last good answer, and treat repeated failure as a warning (logged, reported by chat status), not an exit.
2. Audit the other subprocess calls on chat run's long-lived path (herdr, git, helpers) for the same timeout-is-fatal pattern and fix them the same way where safe.
3. Docs: the chat bridge setup (userguide, setup skill) recommends Restart=on-failure for the bridge unit.
4. Tests: a fake git that hangs past the timeout; chat run keeps running and recovers when git answers again.
