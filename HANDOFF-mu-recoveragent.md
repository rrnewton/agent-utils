# mu-recoveragent handoff — #1559 restore-recovery-agent-plan

Written 2026-11-20 ~11:3x AM ET on coordb's reboot warning (owner rebooting
devbig014 to test crash recovery). No code written yet; nothing to lose except
this context.

## Where I am

- Lane: mu-recoveragent (agentctl lane, lead coordb). Worktree:
  /home/newton/work/dev-agent-utils/worktrees/slots/singleton/.muse/worktrees/recoveragent
  Branch mu-recoveragent/1559, based on github/main 2a74b782 (fetched via
  herdr-run 2026-11-20; FETCH_HEAD == github/main). Worktree git identity set
  (hermit-bot3). Worktree was clean at reboot notice — verify with
  `git status` after restore; no WIP commit was needed if still clean.
- Read in full: brief /tmp/coord014-replies/ui1117/brief-mu-recoveragent.md;
  harness plan ai_docs/plans/2026-11-15-restore-recovery-agent-plan.md
  (phases 0–4); ai_docs/plans/2026-11-12-team-restore-protocol.md;
  skills/team-restore/SKILL.md (primary-checkout copy; HEAD copy still to
  diff) and .claude/skills/harness-team-restore/SKILL.md.
- Launched a READ-ONLY verification workflow "verify-restore-plan-vs-main"
  (6 parallel slices + critic + gap follow-up + synthesis) to re-check the
  plan's what-exists claims at github/main 2a74b782. It had NOT returned when
  the reboot notice arrived and will die with the host. Script persisted
  under this session's workflows/scripts/ dir (session
  01a1f9d2-1b24-79f1-9cb8-1f4152bf02b6). Slices: driver-core (team_driver.rs,
  team_model.rs, CLI wiring, recovery_ladder.rs, agent_watch_recovery.rs);
  manifest-act-verify (team act / manifest existence, restore --dry-run
  schema, team_verify.rs checks, coordinator-ack, chat-health);
  phase0-bugs (#327 bugs 2/3/5/6 verdicts in revive.rs/send/adopt);
  deployment-units (install-units, agentctl-team@ template, watch/turnover
  deferral, live unit listing); oneshot-harnesses (muse/claude/codex exec
  --help evidence + existing one-shot wrappers + profiles.rs);
  trackers-tests-docs (#1414/#1415 status, fake-Herdr/scratch-registry test
  helpers, validation/components.json, harness ai_docs update rules).
  First observations before the workflow: .minibeads/issues/agent-utils-1559.md
  does NOT exist yet (file it with my first commit, per brief); #1414/#1415
  files exist; team_driver.rs exists on main and got lock fixes since
  ba5b3f5e (f9090ae9, 92dfa02f).

## Design change (owner 235 via coordb, 11:32 AM ET) — supersedes plan §3 L2

- Recovery agent is NOT a Herdr-pane agentctl session. It is a series of
  one-shot calls in a loop ("emergency recovery mode", maximal reliability).
- Build (1) tiny wrapper "run an agent with a prompt": try Muse one-shot
  first, then `claude -p`, then `codex exec`; fall through on launch/auth
  failure or timeout; hard bound `timeout -s KILL`; no Herdr/TUI/agentctl
  session machinery, no dependency on services being up; prints final
  answer, logs which harness answered, distinct exit code when all fail.
  Exact flags from each harness's --help only (workflow slice 5 was
  collecting this; re-collect after reboot if results are gone).
- Build (2) boot recovery loop: after deterministic restore, call wrapper
  repeatedly with a recovery prompt carrying driver facts (restore report,
  health checks). Each call may fix one thing via safe idempotent
  agentctl/team commands (i.e. the Phase 1 `team act` allowlist). Loop ends
  on a structured final-JSON-line answer {verdict YES|NO, leads:[{name,
  live, chat_connected, handoff_acked}], next} that the LOOP verifies
  deterministically before accepting YES (lead live + bridge health ok +
  handoff ack). Bounded give-up (N attempts / T minutes) pages owner via
  liaison/bridge or escalation file.
- Update the 2026-11-15 plan doc in the HARNESS repo
  (/home/newton/work/dev-agent-utils/ai_docs/plans/) to match, with a
  changelog entry per that repo's rules (ai_docs/AGENTS.md — workflow slice
  6 was checking how harness changes are committed).
- Owner 234 acceptance bar still stands where compatible: deterministic
  side never blocked on the agent; exactly one restorer per team; no agent
  launched twice; agent finishes only when prior team coordinators are back
  AND connected to chat, then hands off or escalates. Owner 234's "restore
  visibly in a Herdr pane" now conflicts with the no-Herdr-dependency rule
  for the agent path — my phase plan will propose: boot unit = systemd
  service (journal + durable state/log files are the visibility), optional
  best-effort pane mirror once Herdr is up, never a dependency; coordb to
  confirm.

## Next steps (in order, after restore)

1. `git status` in the worktree; re-fetch github/main via herdr-run if the
   host was down long; check whether the verification workflow left any
   result — otherwise resume/re-run it from the persisted script.
2. Corroborate load-bearing findings myself (team_driver states, team act
   absence, one-shot help lines) before writing the phase plan.
3. Draft the SHORT phase plan for coordb: deliverables per landing
   (Phase 1 manifest + `team act`; Phase 2 wrapper + loop + deployment),
   deterministic-vs-agent split, no-reboot testing (scratch registry +
   fake Herdr + fake one-shot harness scripts; drill team), open questions
   (pane visibility; "every prior team coordinator" = this team's lead or
   all host teams?; loop start timing — owner 234 "watch from the start"
   vs plan fast-path vs 235 "after the deterministic restore").
4. Send via `agentctl send coordb` (reply channel given in coordb's
   messages). WAIT for approval. No code before approval.
5. Then land in small gated pieces; file .minibeads/issues/agent-utils-1559.md
   with the first commit. Restore/revive/liveness changes: cold review —
   send coordb the SHA before gating. Report any new persisted registry
   field to coordb before landing. NEVER run a real restore/reboot drill
   on the live host without coordb's explicit OK; scratch registries only.

## Standing rules (from brief)

Network git ONLY via herdr-run from the slot cwd. Every command cwd =
worktree. NEVER git stash. Never touch ~/work/dev-hermit/.agentctl or panes
I didn't create. Gate: GATE_SLOTS=3 python3 scripts/gate.py
--no-local-preflight <reason> -- python3 scripts/validate.py --base
github/main --keep-going > /tmp/mu-recoveragent-gatelog 2>&1 (never pipe;
touch nothing while it runs; check ledger head_sha). cargo fmt --all --
--check clean. New test files classified in validation/components.json.
Land only on coordb LAND NOW. End every turn DONE:/BLOCKED:.
