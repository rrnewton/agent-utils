---
title: 'agentctl-second-enter: one extra Enter about 1s after every TUI delivery'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-16T01:07:55.000000000+00:00
updated_at: 2026-11-16T01:07:55.000000000+00:00
---

# Description

Owner request 104 (2026-11-15, urgent): multiple stuck prompts missing an
Enter key in a coordinator workspace. A lane message was pasted while the
agent was busy, never observed inside the in-lock window, recorded
stranded with no paste label, and left as an unsubmitted placeholder in
the composer; another registry's drain then refused for minutes. The
existing backstops match visible text and exclude placeholder pastes, so
nothing pressed Enter. The owner asked for a deterministic second Enter:
every delivery that types into a TUI pane presses one extra Enter about
1 s after its submit, on by default, so a swallowed first Enter cannot
strand a paste. Related: #1690 pane-queue-linearize (cross-registry
ordering, out of scope here), #1560, #1683.

# Design

Commit 1 (this issue's first landing): in `submit_verified` (Rust
`rs/agentctl/src/submission.rs`, Python `py/agentctl/submission.py`), on
every typing outcome (verified, unverified, stranded, paste-not-observed)
and before the caller releases the pane lock, wait ~1 s through the
injectable runtime clock, re-read the screen, then: skip on an approval,
permission, picker or dialog prompt (existing blocked predicates); press
on an empty composer (verified harmless live for Claude Code, Codex and
Muse in scratch panes, and in the fake-screen model); press when the
composer holds our text or a paste placeholder that appeared after we
typed (Claude newest `[Pasted text #N +M lines]` with M matching, Muse
`[Pasted Content K chars]` with K matching; the pane lock was held the
whole time); skip and record `second_enter_skipped` on provably foreign
text. After pressing, re-verify: a flushed paste is recorded as submitted
by second_enter. `second_enter_at` / `second_enter_skipped` are stamped
on the queue record. Opt out with `AGENTCTL_SECOND_ENTER=0` (documented).
Enabled by `SystemRuntime`; other runtimes default off.

Commit 2: attribute an unattributed placeholder left stranded with no
recorded label, by size, in the drain and the watcher sweep; submit it or
quarantine it as possibly submitted; fix the "nothing was typed" refusal
to say the composer is occupied by an unattributed draft.

# Acceptance Criteria

- Fake Claude screen: paste drawn late as a placeholder, first attempt
  stranded with no Enter sent; the second Enter submits it, the receipt
  says `submitted by second_enter`, one paste only, queue record stamped.
- Verified submit gets exactly one extra Enter on the empty composer;
  foreign draft and blocked dialog get none.
- Both editions behave the same; harness support matrix gains the row.
- `make validate` for the touched paths passes; owner-priority gate lands.

# Notes

Live harmlessness checked 2026-11-15 in own tmux scratch panes: bare
Enter on an empty composer left Claude Code, Codex and Muse screens
unchanged (no turn started). Commit 1 lands first, fast; commit 2 follows
on the same branch naming this issue.
