---
title: 'chat-reply-capture-miss: a well-formed reply block from an agent with no session binding was never captured'
status: open
priority: 0
assignee: cl-start@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T18:07:43.775996897+00:00
updated_at: 2026-11-02T20:31:06.383913247+00:00
---

# Description

[opus 5.5] 2026-11-02 18:00:20Z on one host, a consumer team's bridge (build f0ec02a1, --offer-reply-command): the coordinator (a Claude session) printed a well-formed <CHAT_REPLY_N> block in a text-only message, but the bridge never captured it: the request stayed 'delivered', reply_count 0, nothing posted, no alarm. The agent record had no native session (native_session empty), so the transcript reader (#710) had nothing to read and screen capture missed it. Asks: (1) when an agent has no session binding, chat health says so and the bridge discovers and binds the session (as #741 does for revive: cwd plus last activity, confirmed by content); (2) the bridge alarms (coordinator and owner), not stays silent, when a delivered request gets no captured reply while the agent's transcript shows a CHAT_REPLY_<alias> block for it. Tests for both.

# Notes

[opus 5.5] Same host and bridge build, 18:00:36Z: the bridge HUNG while submitting a request into a busy Claude pane. Its main loop stopped (no log after 17:54Z, no replies sent) while the process stayed alive, and chat health said only DEGRADED. A restart at 18:12Z fixed it. Asks: (3) every blocking step of the main loop has a bound (time out and continue), a stalled loop is detected by a watchdog heartbeat and reported as a distinct health state (not DEGRADED), and the service restarts itself on a stalled loop. Evidence to follow from the other host.

[opus 5.5] Also (owner saw it): the same request got TWO posts 2 s apart after the bridge restart. Reply 1 came from `chat reply N --file` (text taken from the transcript; Markdown table correct); reply 2 was "captured from the coordinator's pane" and its table was mangled by screen rendering. Asks: (4) dedupe: a reply ID already answered with `chat reply` (or any reply with the same text) is never also posted from a capture; (5) pane capture is the last resort: never use it when the transcript is known (the same pass took the next request from the transcript), and prefer refusing with an alarm over posting mangled tables.

[opus 5.5] Lead 2026-11-02: P0, right after #778 (ahead of #781). Slices, each landed alone so bridges can roll forward one at a time: (a) main-loop hang / head-of-line blocking with a stall watchdog; (b) capture-miss alarm plus session discovery; (c) dedupe; (d) last-resort pane fallback.

[opus 5.5] Slice (a) delivered (main-loop hang): the bridge passed no delivery deadline, so each delivery took the registry's hard send deadline (#702), 10 minutes by default. While another sender held the coordinator's queue delivery lock or the host-wide target lock, one request held the whole owner loop that long, which fits the ~12 min hang. The host had a pile of stuck `agentctl send coordinator` processes that day. Now:
- Every bridge delivery waits at most 5 s for those locks and ends `--ready-timeout` + 30 s after it starts. A busy lock leaves the request pending for the next pass.
- The owner loop's longest single wait is 30 s.
- The loop beats a heartbeat each round and rewrites `owner-loop-alive` in the state directory.
- A watchdog thread (`chat run --stall-restart-after`, default 5m, 0 off) treats a loop with no round for the bound as stalled. It logs the stall, records it as the owner loop's failure (class `stalled`), and stops the service, which cancels the Herdr call the loop waits on. If the loop has not ended 30 s later, it exits 70 itself; either way `chat run` exits nonzero, so `Restart=on-failure` starts it again.
- `chat health` reports a running service whose loop has not turned for its bound as STALLED (exit 1, as DOWN), not DEGRADED. `services refresh` treats STALLED as not up.
- Tests: the stalled health verdict and its boundaries, the stall verdict, the watchdog stopping a stalled service and recording it, a turning loop left alone with its alive record, and `chat run`'s bounded delivery options and flag. Slices (b)-(d) remain open.


[opus 5.5] Slice (b) delivered (session discovery, binding, capture-miss alarm):
- When the record names no conversation, the transcript reader (chat_transcript.rs) discovers the transcript: first through the harness process whose environment names the coordinator's Herdr pane, then, for Claude Code, as the newest of the 8 most recently written transcripts in the coordinator's project directory whose last 8 MiB mention a delivered, unanswered request's reply ID. The project lister is shared with #741's revive discovery (harness::claude::project_transcripts / config_roots); revive is unchanged.
- A conversation found through a live process is bound into a record that names none through bind_session itself (same checks and record). Only the locks differ: the bridge tries them and never waits, so a busy record is bound at a later scan. Content-only discovery is read, never bound.
- At each scan the bridge looks for capture misses: a request delivered and not answered whose reply block the transcript has held for 2 minutes or more. Each is logged `CAPTURE MISS`, posted once to the channel as a `[chat-bridge WARNING]`, and told once to the coordinator as a reply reminder is typed (only while it is ready, never behind another prompt), with the `chat reply` command; a notice it was not given is retried at most every minute while the miss lasts.
- The bridge records where reply text comes from, and the current misses, in `reply-source.json`. `chat health` reports DEGRADED when the target's record names no native session and its transcript was discovered or not found, and when reply blocks were never captured.
- Tests: discovery through the pane process and by content (never bound), the shared reply source, the try-lock bind (busy refused, never rebinding), miss detection and its boundaries, one owner warning per miss with the record written, and the health verdicts. Slices (c) and (d) remain open.
