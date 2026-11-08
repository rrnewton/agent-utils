---
title: 'agentctl-muse-native-session-missing: a Muse lane started after #916 had no native session recorded, so revive refused'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-06T18:16:13.000000000+00:00
updated_at: 2026-11-08T01:57:51.000000000+00:00
closed_at: 2026-11-08T01:57:51.000000000+00:00
---

# Description

A Muse lane started on a harness checkout that already contains #916 muse-native-session-id had no native conversation id in its record. After a reboot, `agentctl revive` refused it with "record has no native conversation ID". The coordinator found the session by searching Muse's session directory for the lane's slot path.

# Design

- Find why #916's capture missed this start, for example a timing window or a start path that skips the capture, and fix the capture.
- Revive fallback: when the record has no native id, look in Muse's session store for sessions whose working directory is the record's cwd or slot. Use the match only if exactly one is found, record it, and say so in the revive result. With zero or several matches, refuse and list the candidates.
- Make this case visible before a reboot needs it: status/doctor flags a live Muse record that has no native id.
- One session, every reader (scope added 2026-11-06): after `anchor NAME --replace --session-id ID`, the record's launch_spec.native_session was set (source caller) but the top-level native_session stayed null, including for Claude lanes. anchor --session-id and bind-session must also set the record's top-level native_session (keeping the source), and restart, restore and revive must fall back to launch_spec.native_session when the top-level field is empty, through one effective-session reader. `restart --when-idle` is not on main yet; the fallback belongs in the shared session resolution it will call (restart_with_resume), not in the new command alone.

# Acceptance Criteria

- A test reproduces the missed capture and shows it fixed.
- A test revives a record with no native id through the single-match fallback; zero and several matches are refused with candidates listed.
- A test anchors with --session-id and shows the top-level native_session set with its source kept, in status.
- A test restarts (or revives) a record whose native session lives only in launch_spec and shows it resumes that session.

# Notes

Reported 2026-11-06 by the lead coordinator during a reboot recovery. Related: #916 muse-native-session-id, #928 native-session-collision, #946, #947.

Design (mu-salvage, 2026-11-06, read-only investigation; no code changed yet):

Why the #916 capture missed these Muse lanes. revive::capture_held_session (rs/agentctl/src/subagents/revive.rs) is the only capture, and it runs only at three moments in rs/agentctl/src/subagents.rs: after a start that sends a brief, at adopt, and at anchor-fresh in revive. It depends on held_files(record): the record must pin a process (harness_anchor or custom_process_identity) that is still live and identical, and the walk of that pid's descendants' open files must find a muse .session.lock, within 6 tries 500 ms apart. Exactly one held id is recorded as observed; several become native_session_unknown with candidates; zero is a silent None with nothing recorded. No later path heals a miss: status, probe and doctor never re-run capture, so one start-time miss is permanent for the record's life, which matches all three of today's Muse lanes having no native session. Likely mechanisms, to be confirmed by the reproducing test: (a) the runtime had not opened its session lock within the roughly 3 s capture window and no brief-triggered re-capture followed; (b) the pinned pid's descendant walk missed the runtime process at capture time.

Two pid-keyed sources recovered all three lanes by hand today, and neither needs the fd walk: (1) the dated session tree at $XDG_DATA_HOME/muse/sessions/YYYY/MM/DD/<session-id>/.session.lock, whose lock content names pid=<the runtime's pid>; (2) ~/.local/share/muse/runtime/muse/sessions/<session-id>.json, whose process_generation_hint names pid=<the runtime's pid>. Both key a session id to the exact runtime pid a record pins.

Fix plan. (1) Capture: add pid-keyed discovery beside the fd-held source: scan a bounded window of the dated lock tree and the runtime session jsons for the record's runtime pid (pinned pid or a descendant running the harness), union with fd-held ids, keep the exactly-one rule (several still become native_session_unknown listing candidates), and run the #928 guard so a healed capture cannot double-bind a live record's session. Also re-run this capture for a live muse or codex record that names no session when status or probe already loads the record, saving only when it changes something, so a start miss heals while the lane lives. (2) Revive fallback: when a dead record has no native id, scan the muse session store for sessions whose journal records the record's cwd (or slot path): exactly one match is recorded as observed through the normal bind path and the revive result says it was recovered from the session store by cwd; zero matches refuse with no candidate; several refuse listing the candidate ids and the bind-session remedy. (3) Visibility: status and doctor flag a live muse record with no native session, distinguishing never-captured from unknown-with-candidates, naming the bind-session remedy.

Tests (fixtures already exist in revive.rs tests via HELD_FILES and CLAUDE_CONFIG_ROOTS thread-locals; extend with lock-content and runtime-json fixtures): start capture that misses fd-held but finds the pid-keyed lock records the session; a status heal binds it, and stays unknown naming the owner when another live record holds it (#928 interplay); revive of a record with no native id succeeds on a single cwd match and refuses zero and several matches with candidates listed; the doctor flag appears for a live muse record naming none and not otherwise. Sequencing: implement only after #928 lands, and re-read the capture sites first if #920 changed revive.rs.

## Resolution

[watermelon-full-preview mu-salvage] Landed on github/main as 2955a4cb, e50018df and a5847f4a (replayed onto 2a62654d). Capture now also asks the Muse store (runtime session jsons and dated lock contents keyed by the pinned runtime pid), start captures even without a brief, revive falls back to the single store journal naming the record cwd and refuses several with every candidate named plus the bind-session/anchor remedies, anchor --session-id and bind-session set the top-level native_session, restart/restore/re-pin/revive read effective_native_session, restore and restart_team report refusals per lane with refused_lanes (restore exits a documented 74 when it completed except for refused lanes), and status (Rust and both Python editions) and doctor flag a live muse or codex record with no native session, doctor adding native-session-ambiguous. Clean full gate on the final commit: preflight passed, shared 55/55, exclusive 6/6, validate OK. Unverified: the live status/probe healing variant considered in the design was not implemented (start-time capture plus the doctor pre-check cover the gap), and no live reboot recovery was run against a real registry; the behaviour is covered by fixture tests only.
