# Team topology config shape (#1418 team-topology-checkins)

Status: draft revised after lead review, 2026-11-12. This section publishes
the synced topology config shape early, so every team can draft its own
fragment in parallel and fill in its entries in the same change that enables
check-ins. It implements section 6 of the approved team-restore protocol
plan (`2026-11-12-team-restore-protocol.md`, harness repo) and fits
mu-restdrv's frozen driver, kick and escalation interfaces. No real team or
host names appear here; real entries live in each team's own repo or in the
deployed synced config, never in this repository.

## 1. File locations: per-team fragments

- One fragment per team: `~/.config/agent-utils/topology.d/<team>.json`,
  where `<team>` is the team name (no host). Each fragment holds that team's
  instances on all of its hosts.
- Schema per fragment: `agent-utils-topology/v1` (JSON; no new parser
  dependency).
- Each fragment is owned and landed by that team's lead, from that team's
  own harness repo (section 5). No team edits another team's fragment.
- Sibling reference, not embedded content: the escalation route file of
  #1417, `~/.config/agent-utils/escalation.toml` (see `escalation_config`).
- An optional base file `~/.config/agent-utils/topology.json` (same schema)
  may carry shared defaults only; it declares no team instances.
- Config is synced and identical on every host (plan section 3). Per-host
  runtime state (last-seen times, probed incidents, kicked incidents) is
  never in these files; it lives under `~/.local/state/agentctl/`
  (section 4).
- Fragments are merged at read time. Conflicting declarations are
  reported, never silently merged (the same pattern as the agent-teams
  routes directory).

## 2. Keys

Fragment top level:

| Key | Meaning |
|---|---|
| `schema` | Always `agent-utils-topology/v1`. Unknown schemas are refused. |
| `team` | The team that owns this fragment. Must match the file name. |
| `escalation_config` | Optional. Path to the #1417 escalation route file. Default `~/.config/agent-utils/escalation.toml`. A peer kick that cannot revive its peer escalates through this file; fragments never duplicate route commands. May be omitted (section 3). |
| `checkin_after_silence_seconds` | Optional. Silence after which a check-in is sent. Default `21600` (6 h). Any agent-teams traffic from the peer resets the clock. May be omitted (section 3). |
| `probe_after_silence_seconds` | Optional. Silence after which a peer probe runs. Default `21600` (6 h). May be omitted (section 3). |
| `probe_after_unanswered_seconds` | Optional. Age of an unanswered message after which a peer probe runs. Default `3600` (60 min). May be omitted (section 3). |
| `routes_override` | Optional per-pair route overrides (section 2.1). Absent by default. |
| `instances` | Map of team-instance id to entry (below). A team-instance id is `TEAM@HOST`, with `TEAM` equal to this fragment's `team`. |

Per team-instance entry (the only place the supervision relation is
defined; there is deliberately no `peers` key):

| Key | Meaning |
|---|---|
| `registry` | The team's agentctl registry path on that instance's host (for example `~/work/TEAM/.agentctl`). Used on the target host to derive the driver state location (below). |
| `lead` | Team-instance id of this instance's lead team, or `null`. |
| `subordinates` | List of team-instance ids this instance leads. May be empty. A team may be both a lead (named as another entry's `lead`, or holding a non-empty `subordinates`) and a subordinate (non-null `lead`); a subordinate always probes up and may kick its lead. |
| `coordinator_kind` | `host` or `agentcloud`. An `agentcloud` coordinator answers check-ins but is not modelled as a Herdr pane: no pane check applies to it, and host health comes from the driver state or probe, never from asking it. |

### 2.1 Routes are resolved, not configured

There are no required per-instance route keys. For a probe, kick or
check-in, agent-teams resolves the route from its own per-host route
table, from the watcher's host to the destination host, so a same-host
watcher gets `local` and a cross-host watcher gets that host pair's
declared remote route. Code never hard-codes a route name and never
reads one from the topology fragments by default.

The only exception is the optional top-level `routes_override` map, for a
host pair whose resolved route is wrong for one operation:

```json
"routes_override": {
  "<from-host>-><to-host>": {"probe": "ROUTE", "kick": "ROUTE", "checkin": "ROUTE"}
}
```

Each named route is checked against the routes declared for the
destination host (the same check `agent-teams check` performs); an
undeclared route name is a configuration error, reported before anything
is enabled.

### 2.2 Driver state and `team_key`

`team_key` is only a per-host state-directory name: the first 16 hex of
sha256 of the canonical registry path, giving
`<state-home>/agentctl/team-drivers/<team_key>/state.json` (schema
`agentctl-team-driver/v1`, mu-restdrv frozen). It is computed from the
canonical registry path **on the target host**, because home directories
can differ per host; a watcher never computes or carries a peer's
`team_key`. Kicked-incident memory (section 4) is keyed by `TEAM@HOST`
(the team-instance id), never by `team_key`.

### 2.3 Shared defaults and precedence

Fragments may omit every top-level default (`escalation_config` and the
three intervals). The effective value is taken from, in order: the
watching team's own fragment, then the optional `topology.json` base,
then the built-in defaults above. The defaults govern the watcher's own
behaviour (when it checks in, probes and escalates), so each team's own
fragment is its policy: two teams with different intervals is not a
conflict, and no cross-fragment comparison of defaults is made. Conflict
reporting covers instance declarations only (section 2.4).

### 2.4 Consistency checks

A checker enforces, before anything is enabled: every fragment's `team`
matches its file name; every instance id's team matches its fragment's
`team`; no instance is declared twice (in one or two fragments); every
`lead` and `subordinates` id is declared in some fragment;
no entry names itself; if entry A lists B under `subordinates`, B's
`lead` should name A (a mismatch is reported, not silently repaired);
every `routes_override` route is declared for its destination host.

## 3. Filled example (two placeholder teams, two fragments)

`~/.config/agent-utils/topology.d/team-a.json`:

```json
{
  "schema": "agent-utils-topology/v1",
  "team": "team-a",
  "escalation_config": "~/.config/agent-utils/escalation.toml",
  "checkin_after_silence_seconds": 21600,
  "probe_after_silence_seconds": 21600,
  "probe_after_unanswered_seconds": 3600,
  "instances": {
    "team-a@host-1": {
      "registry": "~/work/team-a/.agentctl",
      "lead": null,
      "subordinates": ["team-b@host-2"],
      "coordinator_kind": "host"
    }
  }
}
```

`~/.config/agent-utils/topology.d/team-b.json` (defaults omitted; its
effective values come from team-b's own fragment if it set them, then
the base, then the built-ins — never from team-a's fragment. Different
values per team are each team's own policy, not a conflict):

```json
{
  "schema": "agent-utils-topology/v1",
  "team": "team-b",
  "instances": {
    "team-b@host-2": {
      "registry": "~/work/team-b/.agentctl",
      "lead": "team-a@host-1",
      "subordinates": [],
      "coordinator_kind": "agentcloud"
    }
  }
}
```

Reading: `team-a@host-1` leads `team-b@host-2`; `team-b@host-2` is its
subordinate and still probes and may kick upward. To make a team both lead
and subordinate, give an instance a non-null `lead` and a non-empty
`subordinates` list; the shape already allows it with no extra key. No
route names appear: agent-teams resolves them per host pair, and neither
fragment needs a `routes_override` unless a resolved route proves wrong.

## 4. How the shape is used (summary; full behaviour is #1418)

- **Check-in.** If a team has heard nothing from a watched peer (its lead
  or any subordinate) for `checkin_after_silence_seconds`, its driver
  sends a check-in; the peer's lead answers in one line (healthy, plus
  what is running). Any agent-teams traffic counts as hearing from the
  peer.
- **Probe.** After `probe_after_silence_seconds` of silence, or a message
  unanswered for `probe_after_unanswered_seconds`, the watcher reads the
  peer's driver state (`agentctl-team-driver/v1`) and health. It never
  asks an agent. A peer reported as deliberately held (for example a
  turnover pin) reads as held, not unhealthy.
- **Kick.** If the probe finds the peer unhealthy or its lead
  unreachable, the watcher kicks the peer's driver once (`agentctl team
  kick`; idempotent, #1414). Exactly once per incident: the incident id
  comes from the peer driver state (mu-restdrv frozen `incident`), and
  the kicker records kicked incidents in its own per-host state, keyed
  by `TEAM@HOST` (section 2.2), so several probers still produce one
  kick and one notification. A kick revives; it never starts gates, does
  no slot or bridge surgery, and never runs slot recovery or removal on
  a peer.
- **Notify.** Once per incident, to the kicker's own lead and to the
  owner. If the kick cannot revive the peer, escalation goes through
  `escalation_config` (#1417: ordered plain commands, incident text on
  stdin, `{host}`/`{team}`/`{incident}` substituted, first exit 0
  delivers, at most one reminder per hour, one resolved message).
- **Ownership.** After a handoff, the kicked team's lead owns any
  leftover checklist. The kicker stops at kick plus notify.

## 5. Deployment from one source of truth per team

Dotsync is last-writer-wins, so a deployed fragment must never be edited
in place on a host. Each fragment's single source of truth is the
checked-in copy in its own team's harness repo; that team's lead reviews
and lands changes there first, then dotsync deploys the identical file
to `~/.config/agent-utils/topology.d/<team>.json` on every host. Hosts
never originate entries, and no team lands another team's fragment, so
two teams can draft and land their fragments in parallel without ever
writing the same file. A dotsync conflict therefore means two edits
raced inside one team's own repo, and is resolved there, not on a host.
Runtime state that must differ per host (last-seen, last probe, kicked
incidents) is kept only under `~/.local/state/agentctl/` and is never
synced, so deployment can overwrite any fragment at any time without
losing incident memory.

## 6. What the other team needs from this shape

A team can draft its fragment from section 3 today, with placeholder
defaults omitted. It deploys the fragment only after the schema and
checker land (#1418 implementation), because the checker is what reports
conflicting declarations instead of letting them merge
silently.
