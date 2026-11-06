# Lessons from the manual restarts of 2026-11-06 -> R1/R2 requirements and tests

Input to `agentctl restart --when-idle` (R1), `restart --team` (R2), and
item 5 restore-in-place. Based on the harness repo's `bin/restart-inplace`
(the interim manual tool) and the day's restarts.

1. Background work dies on exit. Claude shows "Exit and stop tasks / Move
   to background and exit / Stay"; Muse exits without warning and kills
   backgrounded commands. Every gate/monitor a lane backgrounded is a child
   of its harness, so a restart loses it. Hence the RESTART-READY handshake:
   the agent finishes/stops background jobs, notes what to re-arm, ends its
   turn; then restart; then a "restarted, re-arm now" notice covering
   background shells, Monitors, and session-only crons (they never survive).
   R1 test: a lane with a background job is refused (or waits) naming it.
2. Muse ignores a typed /exit; Ctrl-C twice exits it. Claude /exit works.
   Exit sequence is per-harness and must be verified by the bare-shell
   proof, never assumed. R1 test per harness.
3. A restarted harness loses its Herdr agent name; agentctl refuses
   delivery while it is gone. Restart must restore the name (by recorded
   pane_id + terminal_id when the name lookup fails), then anchor --replace
   (pid changes, terminal stays). Test: name gone -> restart restores it.
4. The wrkslots slot owner pid goes stale; restart runs transfer-owner to
   the new pid, or the report says it is owed. Test asserts the transfer.
5. Records without native_session take the session from the running argv
   (--resume / resume ID); refuse when it contradicts the record.
6. Notices typed while the agent is busy return "possibly submitted";
   restart and re-arm notices wait for idle. Test: notice only at idle.
7. Resumed Muse lanes needed anchor --replace by hand (17 lanes); R1/R2
   re-pin automatically (item 2's predicate).
8. agentcloud: halt and resume by session id via agentcloudctl (R3);
   revive refuses remote agentcloud today. Codex: `codex resume ID`.
9. A coordinator cannot restart itself from inside its own session; it
   needs an external helper (systemd-run or another agent) that waits for
   idle, exits, relaunches, re-anchors. R2 documents/tests this path.
10. Restart must be atomic or resumable: a restart journal like revive's,
    so a stopped restart leaves a known phase, never a half state.
11. Wait for idle twice, seconds apart (status flaps); never restart with
    queued input in flight. Test: flapping status delays, queue blocks.
12. Finished lanes are stopped, not restarted; R2 skips them with a reason.
13. Internal subagents die with the harness too. A lane declared
    RESTART-READY while its internal subagent was still running, so its
    restart had to be deferred. RESTART-READY means no background commands
    AND no running internal subagents; R1 checks for them where the harness
    lets it (child processes / task state) and waits.
14. A Muse lane with a running watch task (Muse-internal) keeps the
    pane "working", so wait-for-idle never fires. RESTART-READY must
    include stopping watch tasks, and R1 reports "waiting on a watch
    task" rather than timing out silently.
15. Model API outages kill turns mid-way; a lane that crashed out of its
    turn never says RESTART-READY itself. R2 treats a lane that is idle
    after a failed turn (no background work) as restartable, with a nudge.
16. Muse asks "Local work is still active, exit anyway?" on Ctrl-C when
    the session has scheduled tasks or a paused goal; the restart must
    detect that, choose Stay, and require the re-arm list (cron lines,
    goal text) first, then restore those after resume.
17. Herdr builds differ in key names (one rejects "C-c"; "ctrl+c" works);
    use the canonical spelling and handle refusal. A re-arm note typed
    into a just-restarted lane can strand unsubmitted, so the post-restart
    notice must go through agentctl's drain with submission evidence.
18. (Ours) A restart kills session-only crons and background validation
    gates: the RESTART-READY note must list every background job, monitor,
    and cron to re-arm, and the restarted lane re-arms before new work.
19. (Ours) Manual recovery kept colliding on registry identity: Herdr
    name, pane census, and foreground argv disagreed after reboot; the
    argv-derived session (#947) and pane_id+terminal_id lookup are the
    tie-breakers, with any reported session staying authoritative.
