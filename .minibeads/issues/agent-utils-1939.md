---
title: 'agentctl-codex-replace-goal-picker: recognise Codex Replace goal picker in prompts/answer and auto-answer only agentctl-set goals'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
- codex
- goals
created_at: 2026-11-20T00:50:00-05:00
updated_at: 2026-11-20T00:50:00-05:00
---

# Description

[watermelon-full-preview] A Codex lane sat 75 minutes at Codex's
"Replace goal?" picker ("1. Replace current goal / 2. Cancel", option
1 highlighted), triggered by a coordinator's post-reboot goal restore,
with 7 prompts queued behind it. `agentctl prompts NAME` returned
{question: null, state: menu_or_picker}; `agentctl answer --option 1
NAME` refused ("no supported blocking question is visible");
`agentctl send` stayed pending. Related to the restore work in #1934
agentctl-restore-muse-sessions-and-timeout (stuck pickers triggered
by restore).

# Design

- Make `prompts` and `answer` recognise this picker with a strict
  text match, like the hooks trust dialog: the exact "Replace goal?"
  title, the "New objective:" line, both numbered options, and the
  confirm footer, with the selection read from the highlighted row.
  Answering re-reads the screen before every key (the existing
  verified-answering machinery already does).
- When the goal being offered was set by agentctl's own goal restore
  or delivery (a registered goal, or the raw `/goal` command that
  flow typed), that flow answers "Replace current goal" itself,
  verifying the offered goal text matches what it sent (a cut or
  wrapped showing must be a prefix of it). Any other picker is only
  ever reported through `prompts`, never auto-answered.
- Update the harness support matrix (Codex row) in the same change.
- Tests use recorded pane text.

# Acceptance Criteria

- `agentctl prompts NAME` on the recorded picker screen returns the
  question with both options and the selection, and
  `agentctl answer NAME --option 1` / `--option 2` answer it with a
  re-read before each key.
- A goal agentctl itself delivered, whose offered text matches what
  it sent, is confirmed by that delivery flow; an offered goal that
  differs, or with Cancel selected, is never auto-confirmed.
- An ordinary send that happens to show matching text gains no
  confirmation authority beyond the existing goal paths.
- The harness support matrix Codex cells match the code.

# Notes

[watermelon-full-preview] Lane mu-restorefix/1939, follow-on to
#1934 agentctl-restore-muse-sessions-and-timeout, coordinated by
coordb. Recorded picker text: "Replace goal?" / "New objective:
<objective>" / "› 1. Replace current goal  Set the new objective and
start it now" / "2. Cancel  Keep the current goal" / "Press enter
to confirm or esc to go back".

## Resolution

[watermelon-full-preview] Landed as 738e3333 on main. Codex's
Replace-goal picker is recognised by strict recorded text in both
editions (last block on screen, footer ending it, selection from
the highlighted row), so prompts names it and answer chooses in
it. A goal delivery flow confirms only its own offered goal
(exact, or a cut showing cut only on an ellipsis with whole-word
agreement), refuses before typing over any picker already open,
and never cancels a picker it did not trigger. The cross-edition
differential passes (104 paired checks).
