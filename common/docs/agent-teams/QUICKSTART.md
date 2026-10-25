# agent-teams quickstart

**Problem.** Several teams of agents run on several hosts, each led by a
coordinator. Reaching another team's coordinator today means knowing by hand
which host it is on and how to type into it, and that knowledge goes stale. A
retry after an unclear outcome then delivers the same message twice.

**What agent-teams does.** A team's coordinator (or a registered service)
sends a message to another team's coordinator with one command. The message
carries one ID on every route; the receiving host's own `agent-teams accept`
claims that ID in the team's ledger before handing it to a local deliverer,
so a second route or a retry can never deliver it twice. Nothing drives a
remote host's terminal: the receiving host does its own typing.

**Dependencies.** One Rust binary per host. No Herdr, no agentctl and no chat
bridge are required: with the `hook-inbox` deliverer a harness hook prints
messages at the next turn boundary. With agentctl, the `agentctl-queue`
deliverer enqueues into the coordinator's queue and wakes idle agents.

## Install

```bash
cargo build --release --manifest-path rs/Cargo.toml -p agent-teams
install -m 0755 rs/target/release/agent-teams ~/.local/bin/agent-teams
```

## Set up two teams on one host

```bash
mkdir -p ~/.config/agent-teams/routes.d
cat > ~/.config/agent-teams/teams.json <<'JSON'
{
  "schema": "agent-teams-teams/v1",
  "comment": "Owner-level routing hints; each host's declaration is authoritative.",
  "hosts": {"host-a": {"routes": ["local"]}},
  "teams": {
    "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
    "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
  }
}
JSON
# From inside each coordinator session:
agent-teams init tools --coordinator-self --deliver hook-inbox --accepts-from sim
agent-teams init sim --coordinator-self --deliver hook-inbox --accepts-from tools
agent-teams check
```

## Send

```bash
# From inside the sim coordinator:
agent-teams send tools --text "Please hold landing until the mirror is repaired." \
  --kind hold --topic ci-hold
# The tools coordinator sees it at its next turn boundary (hook-inbox) with
# its ID and a reply command. Re-running the same send is a retry, not a copy.
agent-teams reply tm-sim-... --text "Holding; say when clear."
```

Workers never send: a worker that needs another team asks its coordinator.
A lieutenant (an agent that manages others but is not the registry's
designated coordinator) also asks its coordinator; only the root coordinator
and registered services send cross-team.

See `agent-teams userguide` for routes (ssh, managed links, the star
topology), exits, and the exactly-once rules.
