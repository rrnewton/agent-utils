# herdr-run

Run an **allowlisted** command in a Herdr terminal pane that lives **outside** an AI agent's
sandbox, and get its real stdout, stderr, and exit code back.

```
herdr-run release-agent 'with-proxy git ls-remote origin main'
```

A sandboxed coding agent often cannot reach the network, so `git fetch`, `git push`, and `gh` fail
even though the agent legitimately needs them to publish its work. The Herdr terminal server runs
outside that confinement, so a shell in one of its panes is not confined either. `herdr-run` turns
that into one narrow, audited door instead of an ad-hoc pile of keystroke injection.

> **This tool deliberately crosses a security boundary.** Keep the allowlist as small as your
> project can tolerate, and read the threat model in the user guide before widening it.

## What it does

- **Idempotent bring-up.** Finds or creates the Herdr workspace, the per-agent tab, and its pane.
  Running it twice does the setup work once; running it against half-built state completes only the
  missing part.
- **Allowlist by construction.** The command is split into argv, the program name is checked against
  a per-project allowlist, and every token is re-quoted before it reaches a shell - so shell
  metacharacters become literal arguments instead of a second command.
- **Real results, not screen scraping.** The pane redirects into a spool directory, so you get exact
  bytes and a genuine exit code rather than text recovered from a hard-wrapping, finite,
  ANSI-laden terminal that shows no exit status at all.
- **Conservative readiness.** Two independent signals must agree before anything is typed into a
  terminal a human may also be using, and the check fails closed.
- **Audited.** Every attempt - including every refusal - appends one JSON line to a durable log.

## Configuration

Policy is per project, in the nearest `.herdr-run.yaml`. Every field has a working default, so a
project can adopt the tool with no configuration at all:

```yaml
workspace: agent-cmds     # Herdr workspace holding this project's command tabs
tab_name: "{agent}"       # one tab per agent
allow: [git, gh]          # THE ALLOWLIST - keep it small
prefixes: [with-proxy]    # wrappers that may precede an allowlisted program
spool_dir: .herdr-run     # git-ignored: holds command output, not source
```

## Quick reference

```
herdr-run <agent> '<command>'    run the command; exits with the command's own exit code
herdr-run check '<command>'      policy only: allowed or refused. Touches no pane.
herdr-run target                 resolve/bring up the pane; print ids and readiness
herdr-run config                 print the effective configuration
herdr-run doctor                 verify the sandbox-crossing actually works, both directions
herdr-run userguide              the full user guide
```

Requires the `herdr` terminal multiplexer on `PATH`. `PyYAML` is optional and needed only when a
configuration file is present.

Full documentation: `herdr-run userguide`.
