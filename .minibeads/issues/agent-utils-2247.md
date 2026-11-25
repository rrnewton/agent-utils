---
title: 'agentctl-headless-serve-muse-argv: the serve transport passes exec-only flags to muse serve (it exits 2)'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- headless
assignee: cl-museserve
created_at: 2026-11-25T12:55:00-05:00
updated_at: 2026-11-25T12:55:00-05:00
---

# Description

A headless Muse started on the default serve transport (`agentctl start NAME --mode headless --profile P --cwd ... --file brief`, where profile P is a Muse profile whose argv carries `--yolo --dangerously-enable-internet-mode --disable-request-user-input` and whose declared effort is `max`) never runs. The harness exits 2 on every restart until the wrapper logs "giving up after 5 restarts in 600 s" and stops with `restart-limit`. Its briefs stay queued. The same start with `--transport exec` works.

The serve argv as launched was, flag names in order: `muse serve --trust-workspace --disable-request-user-input --dangerously-enable-internet-mode --yolo --disable-request-user-input --model <m>`. It has no reasoning-effort flag. Exec lanes launch `muse exec ...` and do carry `--reasoning-effort` and `--session-id`.

Run by hand, Muse prints this on stderr and exits 2: "muse serve: --yolo is not a serve option; approval mode is selected over the wire (session/start.approvalMode, session/setApprovalMode) and constructed nowhere", then "usage: muse serve [OPTIONS]". Serve without `--yolo` keeps serving, and serve with only `--dangerously-enable-internet-mode` is fine. Muse 1.4.27 also refuses `--disable-approval`, `--approval-mode`, `--reasoning-effort` and `--json` on serve, and refuses a repeated `--model`.

The adapter (`rs/agentctl/src/headless/muse.rs`) keeps `--model`, drops `--effort` and passes every other operator argument through to `muse serve` unchanged. A profile written for the interactive TUI or for exec therefore puts exec-only flags on the serve command line. The cause stayed invisible because the wrapper keeps no harness stderr (#2248).

# Design

- Build the serve argv from a serve-specific allowlist: the options `muse serve --help` lists, plus the muse launcher's own options (such as `--dangerously-enable-internet-mode`), which the launcher takes before it runs the binary. Every other argument is left off, and one `error` event at spawn (class `harness_arguments`) names what was left off.
- Approval-mode flags go on the wire, not in argv. `--yolo` and `--disable-approval` select `allowAll`; `--yolo` also adds serve's `--disable-sandbox`, because yolo means no approval and no sandbox. `--approval-mode on-request|untrusted|never` selects `onRequest|promptUnmatched|denyUnmatched`. With no approval flag the mode stays as it is today. The mode travels in `session/start.approvalMode`. `session/resume` has no approval member, so after a resume the mode is sent as `session/setApprovalMode` once the session has opened.
- Effort: serve takes no effort flag, so the declared effort goes on the wire. `session/setReasoningEffort` sets the session's standing default once the session has opened, and `turn/start.reasoningEffort` still carries it on every turn. A refused setting is logged as an `error` event and never restarts the harness.
- Repeated flags are deduplicated. A bare flag is kept once. A single-valued option keeps its last value, which is the declared model agentctl appends. A repeated `-c KEY=VALUE` keeps its distinct pairs.

# Acceptance Criteria

- Hermetic Rust tests cover the serve argv built for a profile with `--yolo --dangerously-enable-internet-mode --disable-request-user-input` and effort `max`. The argv has no `--yolo` and no duplicate flags. The session start selects `allowAll` on the wire, and the effort is carried at session open and on each turn.
- A resumed session gets `session/setApprovalMode` after it opens.
- The exec transport's arguments are unchanged.
- Every flag kept on the serve argv is in `muse serve --help`'s set or is a muse launcher option.

# Notes

Reported by a consumer's lead from a bounded repro on their host, now cleaned up. Related: #1234 headless-muse (the adapter), #1216 agentctl-effort-option (effort always set by agentctl), #2248 agentctl-headless-wrapper-keeps-harness-stderr.
