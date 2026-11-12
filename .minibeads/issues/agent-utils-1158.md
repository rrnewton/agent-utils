---
title: 'coordinator-never-blocks: chat-driven coordinators cannot park on an in-TUI question, and a blocked coordinator pages the owner'
status: closed
priority: 1
issue_type: feature
assignee: mu-inbox
labels:
- agentctl
created_at: 2026-11-09T17:05:54.257430709+00:00
updated_at: 2026-11-12T12:51:21.900078164+00:00
closed_at: 2026-11-12T12:51:21.900078093+00:00
---

# Description

Owner request 2026-11-09 (reply 248), via the lead coordinator. On 2026-11-09 a chat-driven Claude TUI coordinator blocked on a Claude Code AskUserQuestion (whether to sudo rm a 128G swapfile). Everything queued behind it until a human answered at the terminal. A chat-driven coordinator must never block on an in-TUI question: it should ask the owner over chat and keep working.

Immediate local mitigations already applied by the lead coordinator (not this issue): AskUserQuestion denied in the user-wide Claude settings and in both harnesses' workspace Claude settings; --disable-request-user-input appended to every local Muse launch profile 2026-11-09; Codex blocking request_user_input (Plan mode regardless of the features.default_mode_request_user_input pin) disabled host-wide 2026-11-09 via [tools.experimental_request_user_input] enabled=false, verified with codex debug prompt-input (Default render clean, proven; Plan inferred from the shared tool registry). This issue generalizes through agentctl: (1) coordinator launch profiles/launch must disable each harness's blocking question tool by construction - Claude Code AskUserQuestion, Muse request_user_input (muse --disable-request-user-input withholds it for the run; no persistent Muse settings key exists), and Codex request_user_input (tools.experimental_request_user_input; the features pin alone leaves Plan mode exposed). (2) The agentctl skill/userguide states the rule: a chat-driven coordinator asks the owner over chat and keeps working; it never parks its turn on a terminal question. (3) When a coordinator lane nevertheless sits at a blocking prompt, the #675 watch-blocked-fast-alerts path must page the OWNER (chat/owner alarm route), within minutes - not merely queue a notice to the blocked coordinator itself. The 2026-11-09 morning evidence (whether the alert fired and reached the owner) is recorded in the resolution or a linked harness report.

# Acceptance Criteria

- agentctl documents (userguide + embedded skill) how a coordinator profile disables the blocking question tool per harness, with the exact flag/settings for Claude, Muse, and Codex; profile validation or launch warns when a coordinator-profile launch would retain an enabled blocking question tool, where agentctl can detect it.
- A blocked-coordinator alert reaches the owner route within minutes of detection even when the blocked lane is the designated coordinator; covered by a test with a fake clock / fake pane showing the owner route is used and the notice is not only queued to the blocked lane.
- Full gate passes. Deployment to live watchers/bridges stays with the coordinators; this issue lands code + docs and names the SHAs.
