---
title: 'codex-footer-composer: agentctl reads Codex 0.169''s status footer as unsubmitted composer text and refuses to send'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-15T12:08:06.535260796+00:00
updated_at: 2026-11-15T12:08:06.535260796+00:00
---

# Description

## What happens
`agentctl send` and `agentctl start --file` to an idle Codex 0.169.0 pane refuse to type. The error is "composer already holds unsubmitted text" and the quoted "text" is Codex's status FOOTER, not composer input:

    error: message 01794744462329112682-48065 remains pending without consuming a retry attempt:
    pane wD:p5 composer already holds unsubmitted text
    "GPT-6.1-Sol medium · /deepscry-newton-fedora-MZ01GC9H · Read coordinator brief"; refusing to append to it; nothing was typed

Before the first turn, the footer is "GPT-6.1-Sol medium · /deepscry-newton-fedora-MZ01GC9H". After a turn, Codex appends the thread title ("· Read coordinator brief"). Both forms are misread.

## Screen at the time (idle, empty composer)

    › Ask Codex to do anything

      GPT-6.1-Sol medium · /deepscry-newton-fedora-MZ01GC9H · Read coordinator brief
      ← for agents · ? for shortcuts                       ⚠ 2 warnings · f2 to view

## Repro
- agent-utils main ba5b3f5e9; agentctl rust source=9fecd258eb18 binary=c7f10197fb37. It also reproduces on the much older 41705620.
- codex-cli 0.169.0, model gpt-6.1-sol, launched by `agentctl start relay --harness codex --model gpt-6.1-sol --harness-arg=--dangerously-bypass-approvals-and-sandbox --harness-arg=--no-daemon --workspace-id wD --file brief.md` in Herdr 0.9.5 (in-container server).
- The start's initial brief stays pending. Every later `agentctl send relay "..."` returns outcome=pending with the error above.

## Second symptom (manual workaround path)
Typing by hand with `herdr pane send-text wD:p5 "<text>"` then `herdr pane send-keys wD:p5 enter`: the FIRST enter is often swallowed, and the prompt sits unsubmitted (the owner saw it stalled). A second `enter` submits it. This may be the same paste-settling race that the unsubmitted-paste backstop (4cbc07e42) covers for Claude/Muse, but not for Codex 0.169.

## Side note (may deserve its own issue)
codex 0.169.0 here refuses to start without `--no-daemon`: "daemon executable not found at /root/.codex/packages/standalone/current/codex". `codex app-server daemon update` returns status "unsupported". agentctl's Codex launch could detect this and add --no-daemon, or report it clearly.

## Expected
The footer line under the composer is not composer input. An idle Codex 0.169 pane accepts `agentctl send`, and the submit key reliably submits.

Reported by the DeepScry newton-fedora coordinator, 2026-11-15 ~7:10am ET.
