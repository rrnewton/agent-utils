# agent-teams

Message another team's coordinator, across hosts, exactly once.

```bash
cargo build --release --manifest-path rs/Cargo.toml -p agent-teams
agent-teams quickstart   # the operator documentation
agent-teams check        # verify declarations, contacts and routes
agent-teams send other-team --file msg.md
```

A team is a coordinator plus the workers it manages, on one host. Only the
root coordinator (proved by process ancestry over the `managed_by` tree) or
a registered service may send, and only to another team's coordinator.
Delivery goes through a `Deliverer` trait (agentctl queue, hook inbox,
command) and transport through a `Route` trait (local, ssh, command), so the
tool needs neither Herdr nor agentctl: a hook-inbox team works with this one
binary per host.

Design: `ai_docs/transient/2026-10-24-agentctl-team-messaging.md`. Related
work: [`RELATED_WORK.md`](RELATED_WORK.md).
