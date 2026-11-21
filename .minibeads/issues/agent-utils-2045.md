---
title: 'agentctl-muse-phantom-background-send: a finished background send stays counted as running in Muse, holding the lane idle-stuck'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- muse
created_at: 2026-11-21T09:08:00-05:00
updated_at: 2026-11-21T09:08:00-05:00
---

# Description

[watermelon-full-preview] A consumer team reports (2026-11-21)
several Muse 1.4.25 lanes whose task list shows `agentctl send
lead-... running 26m+` (one lane has two such entries 18-19 h old)
while no such process runs under the harness.
`agentctl watch-agents` already says "status idle waiting on
background work the screen counts (1 background shell), but no
shell runs under its harness" (#1991
watch-agents-stale-background-tasks). The model's turn has ended
and a queued input entry sits unsubmitted: Muse seems to hold
queued input while it believes background work runs. The likely
trigger is a lane running `agentctl send` to a busy coordinator
without `--queue-only`: Muse moved the send to its background,
and the process later hung, exited or was killed in a way Muse
did not register as an exit for the counted entry. #2036
agentctl-send-timeout-inotify-wait (landed 2f703edc) now bounds
every send wait, so new sends end; the counted entry they leave
behind is this issue.

## Reproduction (scratch Muse 1.4.25 session, 2026-11-21)

[watermelon-full-preview] A scratch Muse session (never another
lane's pane) ran plain `sleep` commands as stand-ins for a long
send. Muse runs a background shell in-process: the child (for
example the `sleep`) is a direct child of the Muse runtime serve
process, in its own process group; there is no intermediate OS
shell to kill. Observed, with the pane's terminals panel
(`main · ...` plus one row per terminal) and `/tasks` read after
each step:

- Normal exit (yield-backgrounded `sleep 90`): the runtime
  registered the exit at once (the model was woken and reported
  `sleep 90 finished, exit 0`; its exec session reports completed)
  and `/tasks` showed "no running or recent work", but the
  terminals-panel row kept reading `running` with its timer still
  counting past 14 minutes, until the session was exited. A
  foreground `echo` in between did not clear it.
- SIGTERM (yield-backgrounded `sleep 240`, killed by literal PID):
  the model reported `exited with signal 15`; the panel row
  cleared, a short time after the completion turn ended.
- SIGKILL (Ctrl+B-backgrounded `sleep 300`, killed by literal
  PID): the model reported `exited with signal 9 (SIGKILL)`; the
  panel row was removed. Termination through the model's own
  background-session terminate also removed the live row.
- Parent death: the only parent is the runtime serve process;
  killing it (SIGKILL, literal PID) printed "The session host
  exited", orphaned the `sleep` to init, and ended the session.
  No counted entry survives a resume: exit plus `muse resume`
  came back with no terminals panel at all, which also cleared
  the normal-exit phantom above.
- A detached child holding the output pipe (`setsid -f sleep`;
  main command exits 0) does not create an entry: Muse tracks
  the main process exit, not pipe EOF alone. Unmanaged shell
  backgrounding (`(sleep &)`) is refused by the Muse shell tool.
- Queued input: at idle with a phantom row, typed input submitted
  at once (it steered/started a new turn) in every trial; the
  phantom panel row alone did not hold it. The reported
  unsubmitted queued input was not reproduced in the scratch
  session, so its hold is not explained by the stale panel row
  alone (see Design).

So Muse needs no extra signal to notice a direct child's exit:
it noticed every death above immediately. What it does not do
reliably is retire the terminals-panel row for a yield-
backgrounded session that exits normally. agentctl's send can
therefore not "end in a way Muse registers" that fixes the
counted entry: with #2036 the send now always ends at its
deadline, and a normal clean exit is exactly the case whose row
goes stale. The stale row is a Muse-side defect; a short written
report for the owner is the deliverable for that part (filing
upstream needs the owner's OK).

# Design

[watermelon-full-preview]

1. Prevention (default): lane-to-coordinator sends use
   `--queue-only` in the shipped guidance, so a lane's model
   never runs a blocking send that Muse can background. Queue-
   only returns at once (still deadline-bounded since #2036),
   the prompt is durable, and the watcher/drain types it when
   the target is ready. Places: `skills/agentctl/SKILL.md` and
   its embedded copy `rs/agentctl/src/embedded_agentctl_skill.md`
   (kept in sync), the coordinator/brief guidance that tells
   lanes to message the coordinator (`skills/agent-utils-
   coordinator/SKILL.md`, `skills/agent-utils-setup` brief text),
   and the sender header's reply hint only if coordb wants the
   stronger change (it names plain `agentctl send NAME` for
   every harness; changing it affects all reply paths, so it is
   a separate decision). Normal agent-to-agent sends that expect
   delivery proof keep the blocking form.
2. Detection (already shipped): #1991 makes watch-agents report
   the stale count once as info when no process runs under the
   harness and the work list is empty. No new detection is
   proposed in `prompts`/`answer`: those commands recognise TUI
   questions, and there is no question to answer here.
3. Safe clear (verified): there is no `/tasks` key that stops or
   clears a terminal on Muse 1.4.25. The `/tasks` overlay header
   offers only move (arrows/jk), Enter open, Esc close; with the
   normal-exit phantom, `/tasks` is already empty, the panel row
   is not in it, `x` does nothing, Ctrl+C only arms Muse's quit,
   and Enter on the panel row opens no stop action. The verified
   clear is exit plus resume: `/exit` showed no "Exit and stop
   tasks" dialog for the phantom (Muse's own task state already
   knew it was finished), and the resumed session had no counted
   entry. That is exactly `agentctl restart NAME --when-idle`,
   which already refuses when input is queued, handles the
   "Exit and stop tasks" dialog only with
   `--stop-background-tasks`, relaunches resuming the recorded
   session in the same pane, and queues a re-arm notice. The
   proposed change is therefore guidance plus, if coordb wants
   it, one watch-agents info-line addition naming that restart
   as the remedy when the stale condition is first reported —
   never an automatic key-press: agentctl must not press keys
   into a lane on a screen guess, and it must never clear an
   entry while any live process exists under the harness.
   Restart is only offered at an idle turn boundary with an
   empty queue, which restart already enforces.
4. Queued input: do not build a queued-input submitter on the
   phantom theory. If a lane's queued input is genuinely held,
   watch-agents' existing turn-queued / auto-interrupt policy
   (`--auto-deliver-after`, Escape plus submit of the queued
   input) is the shipped mechanism; the scratch trials suggest
   the reported case is the same stale-display family plus a
   turn that had already ended, to be confirmed on a live
   affected lane before any code.

# Acceptance Criteria

[watermelon-full-preview]

- The shipped skill/brief guidance tells a lane messaging its
  coordinator (or any busy managed target where it does not need
  delivery proof) to use `agentctl send --queue-only`, with the
  reason (a blocking send can be backgrounded by the harness and
  leave a stale counted entry) stated in one line.
- watch-agents' stale-background info line, if coordb approves
  the addition, names `agentctl restart NAME --when-idle` (and
  `--stop-background-tasks` only after the agent confirms the
  tasks are stale) as the remedy, exactly once per episode as
  today; a live process under the harness still re-flags as
  before and no keys are ever pressed automatically.
- A Muse lane with a verified-stale counted entry clears it
  with that restart and resumes the same session; a lane with a
  live background process is never told to clear it.
- Focused suites for the touched areas pass; the Muse upstream
  report is delivered to the owner as a written note, not filed.

# Notes

[watermelon-full-preview] Lane mu-musephantom, reserved and
numbered by coordb. Prior work searched before filing: #1991
watch-agents-stale-background-tasks (landed e969db9e) and #2036
agentctl-send-timeout-inotify-wait (landed 2f703edc). No lane,
host or person is named here.
