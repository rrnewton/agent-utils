# agent-utils skills

Thin, agent-facing skills for the `agent-utils` tools. Each `skills/<tool>/SKILL.md` is a short
dispatch pointer: a one-line description of the tool plus the three canonical ways to get usage —
`<tool> quickstart`, `<tool> --help`, and `<tool> --userguide`. The skills deliberately do NOT
duplicate the user guide; they point at the CLI, which is the single source of truth (the full guide
lives once at `common/docs/<tool>/USER_GUIDE.md`, is embedded into each package/crate, and is printed
by `<tool> --userguide`). This keeps exactly one guide and one skill per tool, both DRY.

## Available skills

- `safe-ci-dag-runner/SKILL.md` — run a DAG of CI/build steps under cgroup boxing with memory-aware
  concurrency and learned-estimate planning.
- `tick-hub/SKILL.md` — one scheduled tick funnels many cadenced reminders into machine-readable
  HEALTH/ACTION/NOTE/ERROR lines.

## Linking a skill into an agent

Agent harnesses (e.g. Claude Code) discover skills under a `.claude/skills/` directory. Symlink the
tool skills you want into that directory so they auto-trigger:

```sh
# From the agent's project root (adjust the path to your agent-utils checkout):
mkdir -p .claude/skills
ln -s /path/to/agent-utils/skills/safe-ci-dag-runner .claude/skills/safe-ci-dag-runner
ln -s /path/to/agent-utils/skills/tick-hub          .claude/skills/tick-hub
```

Because each skill points at `<tool> --userguide` (the embedded guide) rather than copying it, the
skill stays tiny and never drifts from the tool's actual documentation. Prerequisite: the tool must
be on `PATH` (via `pip install` / `cargo install`, or the repo's `./bin` after `./setup`).
