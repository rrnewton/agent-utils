---
title: 'agentctl-rename-keep-alias: keep the old name resolving after a rename'
status: closed
priority: 1
issue_type: task
labels:
- agentctl
created_at: 2026-11-17T10:55:29-05:00
updated_at: 2026-11-18T22:31:20-05:00
---

# Description

[watermelon-full-preview] `agentctl rename OLD NEW` moves the registry
entry (and with it the queue dir), the Herdr agent name and the tab
label; the coordinator designation follows by token, and name_history
records the old name. But nothing resolves the OLD name afterwards, so
anything still addressing it fails: lanes' "reply: agentctl send OLD"
headers, scripts and timers, status-file watchers, the chat bridge's
target agent. Owner request (renaming the team leads by host) waits on
this.

Build `agentctl rename OLD NEW --keep-alias DURATION` (for example 14d;
default none = today's behaviour), with alias resolution at the shared
name-resolution point, `agentctl alias list|rm OLD`, and the census in
Notes below.

# Design

An alias record lives in its own subdirectory of the registry,
`REGISTRY/aliases/<old>.json` (schema agentctl-alias/v1: old, new,
token, created, expires), written atomically inside the rename journal:
the journal carries `keep_alias_until`, and `complete_rename` writes
the alias as its last step before removing the journal, so a crash
mid-rename leaves either no alias or a complete one, and the plain
rerun `agentctl rename OLD NEW` finishes both.

Resolution sits at the one funnel every name-taking command already
uses — `ManagedAgents::load`/`load_with_stop_advice` in the Rust
edition and `ManagedAgents._load` in the Python edition — plus the
lifecycle lock and queue addressing that belong to the agent. A live
alias loads the record with the SAME TOKEN; one stderr note per
process (`agentctl: 'OLD' is an alias of 'NEW' until <date ET>; use
NEW`); JSON output carries the resolved name plus an `alias` field. A
replaced token or an expired alias refuses, naming NEW; expired
aliases are pruned lazily. While an alias is live, registering a new
agent under OLD is refused. The chat bridge target, watch-agents
notices and restore already resolve through the same manager, so they
inherit this; `restore --write-agents` and agent-teams' health check
were routed through the token designation instead of bare file names.

# Acceptance Criteria

- Rename with alias, then send/read/status by OLD reach NEW, and
  queued messages survive; alias plus a replaced token refuses; an
  expired alias refuses and is pruned; registering OLD while the alias
  is live refuses; a crash mid-rename via the journal; --json fields.
  Rust and Python editions, with a cross-edition differential group.
- `agentctl alias list|rm OLD` shows and removes aliases.
- Docs: the agentctl skill (rename section); the harness support
  matrix is unchanged because rename is registry-level, identical for
  every harness.

# Notes

[watermelon-full-preview] Census, for a consumer's question: everything
else keyed on an agent NAME that a rename would break. Rename today
updates only the registry directory and record (name_history), the
Herdr agent name (native only), the tab label (only when it still
reads OLD or NEW), and (Rust) the coordinator.json designation by
token. It does NOT update:

- systemd unit files and their ExecStart (headless
  `run-headless OLD`, watch-agents `--coordinator OLD`, scheduled
  entries whose command names OLD): a headless agent cannot even be
  renamed while its unit lives, and a hand rename leaves the unit
  addressing OLD;
- tick-hub notify/dispatch targets: tick-hub itself names no agent,
  but any host dispatch command around it that hardcodes
  `agentctl send OLD` breaks the same way;
- the chat bridge config: `bridge.json` agent_name/agent_label and
  `thread-bindings.json` (name-only routing; the stored token is never
  read for routing), plus per-bridge state directories;
- restore.json (coordinator — which then hard-refuses on mismatch —
  agents, exclude, also_registered names, scheduled commands, units);
- Herdr tab names beyond the one renamed tab: stale labels when the
  pane was missing or the recipient changed, and tab ranking that
  reads bridge/designation names;
- watch config and state: watch-agents CLI/unit arguments,
  `agent-watch-state.json` name-keyed maps, `REGISTRY/.progress/`,
  `landing-queue.json`, `REGISTRY/.waiting/` (the queue inside the
  agent directory does move). watch.json proper contains no agent
  names;
- wrkslots slot records (`agent` field) and any script, timer or
  saved command line that names OLD.

The alias above covers name resolution through agentctl itself (which
includes the chat bridge target, watch-agents notices, restore and
the agent-teams queue deliverer's final hop). agent-teams addresses
team@host and resolves the designation by token through
`agentctl coordinator show`, so it is unaffected by a rename except
where a declaration pins a bare coordinator name; that pin is dropped
at coordinator handover, not by rename.

## Resolution

[watermelon-full-preview] Landed on main as
047d1fe5b5fa531765bc269538439ead57325c7d. `agentctl rename OLD NEW
--keep-alias DURATION` writes `REGISTRY/aliases/<old>.json` inside the
rename journal in both editions; resolution sits at the shared load
point (same token required, one stderr note, JSON alias field, expiry
pruned lazily, registration of OLD refused while live, chained
renames re-pointed, restore accepting an aliased coordinator).
`agentctl alias list|rm` included. Tests: Rust identity/control
(alias, chained, crash, restore cases), Python identity mirrors, and
the cross-edition `rename-alias` differential group (315 paired
checks). Reader skew probed with a pre-change build: older builds
ignore `aliases/` and unaliased resolution is unchanged. The gate
passed on the rebased tree (240b1795) with no overlap flags.
