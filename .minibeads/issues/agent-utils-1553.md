---
title: 'codex-footer-composer: agentctl reads Codex 0.169''s status footer as unsubmitted composer text and refuses to send'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-15T12:08:06.535260796+00:00
updated_at: 2026-11-15T12:36:31.000000000+00:00
---

# Description

## What happens
`agentctl send` and `agentctl start --file` to an idle Codex 0.169.0 pane refuse to type. The error is "composer already holds unsubmitted text" and the quoted "text" is Codex's status FOOTER, not composer input:

    error: message 01794744462329112682-48065 remains pending without consuming a retry attempt:
    pane wD:p5 composer already holds unsubmitted text
    "GPT-6.1-Sol medium · /<workspace-dir> · Read coordinator brief"; refusing to append to it; nothing was typed

Before the first turn, the footer is "GPT-6.1-Sol medium · /<workspace-dir>". After a turn, Codex appends the thread title ("· Read coordinator brief"). Both forms are misread.

## Screen at the time (idle, empty composer)

    › Ask Codex to do anything

      GPT-6.1-Sol medium · /<workspace-dir> · Read coordinator brief
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

Reported by a coordinator on the owner's external machine, 2026-11-15 ~7:10am ET.

# Notes

Redaction: as landed on main in d786df3c, the consuming project's name
in the report above was replaced with neutral labels (`/<workspace-dir>`);
the quoted footer text is otherwise as reported.

## Resolution

Cause: the Codex composer reader took the footer to be the screen's last
populated row and the composer to be every row from the composer marker to
that row. Codex 0.169 draws two footer rows (status, then hint), so the
status row fell inside the composer and read as unsubmitted text. The same
reader in the older build explains why it also reproduced there: it is a
layout change breaking a long-standing single-footer assumption, not a
recent agentctl regression.

Fixed in both editions (Rust `harness/codex.rs`, Python
`agentctl/submission.py`) with a structural rule, not a model-name list:
a status row parses by shape (`<model> <effort> · /path [· title]`, the
field after the first ` · ` naming an absolute or home-relative path),
and when it sits directly above the hint row, blank-separated from the
composer, the footer starts at the status row so the composer ends at
its own boundary. Rows under the composer that no rule can classify now
read as an unrecognisable composer, never as a draft, so an unparseable
future footer cannot refuse a send as held text. Screen fixtures cover
idle 0.169 panes with and without the thread title plus the hint line,
the older single-row layouts, a staged prompt, and a real unsubmitted
draft, which still refuses before typing.

The `--no-daemon` side note is covered on the native Herdr start path:
when a failed launch's pane screen shows `daemon executable not found`,
the error now names the remedy (relaunch with `--no-daemon`, for example
`--harness-arg=--no-daemon`). The flag is reported, never added silently.
Other launch backends keep the generic launch error.

The swallowed-first-Enter symptom is not fixed here: per the owner it is
covered by the harness-agnostic unsubmitted-paste backstop, #1560
(remember the sent text; while a fuzzy
match stays in the pane's lower region over repeated polls, press Enter,
capped and logged), which serves every TUI including unknown harnesses.

Not yet done: the coordinator's services turnover to the landed build.
Until then, live panes still run the old reader.
