---
title: 'anchor-replace-resumed-harness: anchor --replace should pin a hand-resumed Muse or Claude in the recorded pane'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-06T19:07:34.603680928+00:00
updated_at: 2026-11-06T19:37:09.606838894+00:00
---

# Description

After a reboot, lanes resumed by hand (muse <argv> resume <id>, claude --resume <id>) hit a catch-22: anchor --replace refused the dead launch pin and revive refused the occupied pane. anchor --replace now accepts the recorded pane foreground/relay process when its cwd is the recorded cwd and its argv is the recorded launch argv plus one resume selector, recording that session as native_session; unrelated processes stay refused. Both editions.
