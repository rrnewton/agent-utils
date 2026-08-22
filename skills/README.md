# agent-utils skills

These skills help an agent harness discover the repository's tools without copying their manuals.
Each tool skill points to the installed command's quickstart, help, and embedded user guide. The
`pr-landing-operations` skill is a companion process guide for authorized repository operators.

## Available skills

- `safe-ci-dag-runner` — schedule a dependency graph under CPU, memory, and named-resource limits.
- `cpuset-alloc` — reserve disjoint host CPU sets with stale-owner reclamation.
- `tick-hub` — evaluate recurring reminders and health checks from one scheduled tick.
- `pr-landing-planner` — produce an advisory, machine-readable PR landing plan.
- `pr-landing-operations` — validate and execute an authorized landing plan safely.
- `parallel-experiment-runner` — run boxed, resource-bounded concurrent seed sweeps.
- `agent-team-timeline` — archive and visualize coordinator and subagent activity.
- `herdr-run` — run an allowlisted command in a Herdr pane, outside whatever constrains the caller; a sandboxed agent's blocked `git` is one case, not the definition.
- `herdr-agent` — durably deliver and inspect prompts for an interactive agent in a Herdr pane.
- `wrkslots` — manage one isolated Git worktree slot per active agent, record its handoff, and remove it only after verified owner absence.

## Install in an agent harness

Link the desired directory into the skill directory configured by your harness:

```sh
ln -s /path/to/agent-utils/skills/safe-ci-dag-runner /path/to/agent-skills/safe-ci-dag-runner
ln -s /path/to/agent-utils/skills/cpuset-alloc /path/to/agent-skills/cpuset-alloc
ln -s /path/to/agent-utils/skills/tick-hub /path/to/agent-skills/tick-hub
ln -s /path/to/agent-utils/skills/pr-landing-planner /path/to/agent-skills/pr-landing-planner
ln -s /path/to/agent-utils/skills/pr-landing-operations /path/to/agent-skills/pr-landing-operations
ln -s /path/to/agent-utils/skills/parallel-experiment-runner /path/to/agent-skills/parallel-experiment-runner
ln -s /path/to/agent-utils/skills/agent-team-timeline /path/to/agent-skills/agent-team-timeline
ln -s /path/to/agent-utils/skills/herdr-run /path/to/agent-skills/herdr-run
ln -s /path/to/agent-utils/skills/herdr-agent /path/to/agent-skills/herdr-agent
ln -s /path/to/agent-utils/skills/wrkslots /path/to/agent-skills/wrkslots
```

Tool commands must be on `PATH`. From a source checkout, `./setup` creates repository-local
dispatchers under `./bin`. Landing operations additionally require authorization from the consuming
repository's own rules.
