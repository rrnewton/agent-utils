---
title: 'mb-upgrade: run minibeads 0.29.0, whose ready and blocked honour closed blockers'
status: open
priority: 1
issue_type: task
labels:
- tasks
- coordinator
depends_on:
  agent-utils-631: parent-child
created_at: 2026-10-28T14:02:59.895249585+00:00
updated_at: 2026-10-28T14:20:57.103365969+00:00
---

# Description

[opus 5.5] Part of #631 task-tracker-adapters. The mb this harness runs (0.25.0) keeps treating a closed blocker as blocking: B blocked by A, A closed, and mb ready lists nothing while mb blocked still lists B (reproduced in a scratch tracker on 2026-10-28). Upstream fixed it in a210c08, first released in 0.29.0, which is the commit ext/minibeads pins. Until the upgrade, every blocks edge hides its dependent from mb ready for good, so dependency order cannot work.

Done in #631's change: 0.29.0 refused 8 issue files here (non-standard top-level sections '# Outcome' and '# Follow-up' in #21 validation-overhaul and its children #22-#26, and text before the first section in #545 rust-primary-agentctl-chat-readers and #562 rust-primary-wrkslots); they were rewritten without changing their text (sections moved under '# Notes' as '##' headings, or a '# Description' heading added), and .minibeads/.gitignore gained the two transient files 0.29.0 writes (minibeads-transaction.json, .mb-write-*). With those, 0.29.0 reads all issues and 0.25.0 still does.

Remaining (coordinator, a deployment): build mb 0.29.0 from ext/minibeads (git submodule update --init --checkout ext/minibeads; cargo build --release --bin mb, about 20 s), install it at a versioned path, and point the brief template's mb path at it.
