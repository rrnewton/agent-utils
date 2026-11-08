---
title: 'reply-reminder-withheld: an eligible reminder never fires and says nothing, and burst messages stay unanswered'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-08T18:32:55.000000000+00:00
updated_at: 2026-11-08T19:05:01.000000000+00:00
closed_at: 2026-11-08T19:05:01.000000000+00:00
---

# Description

Follow-up to #1046 chat-bridge-duplicate-reply (landed as 00042abc). On a bridge running that build, the reply reminder still never fires. Read-only evidence gathered on the bridge's host: 12 requests are eligible by the code's own filter (delivered after the record started, under 24 hours old, not reminded, no replies stored, not closed, no repoint); the co-ordinator was fully idle for 141 seconds (Herdr done, Claude session `shell` sampled every 3 seconds, screen showing its last turn done with background monitors still running and an empty composer), with many longer idle gaps later that night; yet `reply-reminders.json` gained no entry and `bridge.log` has no reminder lines after startup. Nothing is logged either way: wait reasons log only on change and failures only on transition, so the bridge cannot say why it withheld the reminder.

Two suspects, neither yet proven: the slow-answer deferral interacting with the check cadence (`claude agents --json` takes about 4 seconds on that host, over the 3-second slow-answer bound, so each slow answer defers the next question by one idle period, and any 10-second timeout ends the idle period outright); and the screen side of turn evidence latching (the screen state is set when a prompt is typed and cleared only by a read showing no running-turn marker, and on a bridge run with a minutes-long reconcile interval those reads can be minutes apart once the saturated poll stops, so idle windows shorter than the read gap never register).

Second, from the same bridge's owner: 11 of those 12 "unanswered" requests were overnight bursts — several owner messages in a row in one thread that drew one combined co-ordinator reply attached to a sibling message. Only the replied-to request counts as answered; the rest of the burst counts unanswered forever, reminder candidates included, though a person reading the thread was answered.

# Design

- Log why a due reminder is withheld, at debug or one-shot-per-cause level, naming the gate that held it (not yet idle and which signal disagrees, question deferred, prompts waiting, recovery left work), so the next silent night is legible from `bridge.log` alone. No new logging on the ordinary path.
- Find which gate actually withheld the observed reminders and fix that cause, not the symptom: candidates are the screen-evidence read gap, the slow-answer deferral and timeout handling in the idle query, and the check's early returns that skip idle tracking. The fix keeps every existing protection: no reminder into a running turn, a permission dialog, a typing wait, or over an answer a last read can still store.
- Burst rule, decided here: when a reply is stored in a thread, earlier requests from the same sender in that thread, delivered before the replied-to message and still holding no reply, stop counting as unanswered — one combined answer answers the burst. A request from another sender, in another thread, or delivered after the replied-to message is untouched, and a genuinely unanswered request keeps every protection the reminder gives it. Whether settled requests keep a visible count anywhere follows the counts the bridge already publishes; no new surface.

# Acceptance Criteria

- A regression test with the observed idle screen (turn done, background monitors still running, empty composer) and a slow, over-the-bound Claude answer: the reminder for an eligible fenceless turn fires; without the fix it does not.
- The withheld reason appears in the log at most once per cause while it holds, verified by test.
- Burst tests: several same-sender requests in one thread answered by one reply to a sibling leave none of them reminder candidates, while a later same-sender request, another sender's request in the thread, and a genuinely unanswered control each still earn their reminder.
- The chat suites stay green, and the exact push tree passes the full validation gate, `check_client_names` and `cargo fmt --check`.

# Notes

[watermelon-full-preview] Filed by mu-spark from hands' read-only samples on the affected bridge (idle windows, session statuses, eligibility of the 12). The `inbox/watch.rs` session-status reading noted in #1046 stays out of this change unless the cause found here demands it.

## Resolution

[watermelon-full-preview] Fixed by mu-spark.

Withheld reminders now say so: when eligible requests exist and a check composes nothing, the bridge logs `reply reminders withheld: N eligible request(s): <reason>` once until the reason changes — prompts waiting, a queue hold, Herdr's status, a screen still showing a running turn, the slow-answer deferral, Claude Code's exact session status, or work the pre-reminder read left. Reasons already logged where they happen (failed steps, a pane that cannot be asked) are not repeated.

One gate behind the silence is also fixed: the loop's screen evidence could stay "running" from a read taken before the turn ended, with nothing scheduled to refresh it while Herdr already read idle, so the idle question was never even asked. When Herdr reads idle or done and only that stale screen disagrees, the check now settles it with one fresh read; a screen still showing a running turn withholds exactly as before. The other suspect is exonerated with evidence and pinned by tests: the observed idle screen (turn done, background monitors still running) does not read as a running turn, and slow Claude answers (over the 3-second bound, deferring the next question by an idle period) delay a due reminder by one deferral and no more — `slow_idle_answers_do_not_starve_a_due_reminder` drives that geometry at production scale. Which gate held the affected bridge's night specifically will now be in its log on the first withheld check after refresh; that last confirmation is the one unverified piece, and if the line names a gate these fixes do not cover, it is a new issue with the reason attached rather than another silent night.

Bursts: `reply_reminder_candidates` no longer offers an in-thread request that a later same-sender reply in the same thread answers (delivered after it; a reply to the thread's root counts). The root's own request, a later question, and another thread keep their reminders, each covered by `a_reply_in_a_thread_settles_earlier_burst_requests_from_its_sender`. The settling is at candidacy, so records are untouched and a reply later withdrawn does not leave a request wrongly closed.
