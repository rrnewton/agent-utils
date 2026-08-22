# herdr-run and herdr-agent

Run an **allowlisted** command in a Herdr terminal pane that lives **outside** an AI agent's
sandbox, and get its real stdout, stderr, and exit code back.

```
herdr-run release-agent 'with-proxy git ls-remote origin main'
```

The same package also installs `herdr-agent`, a durable FIFO transport for messaging an
already-running interactive agent without losing or accidentally duplicating a prompt:

```sh
herdr-agent send --session-agent codex --session "$CODEX_SESSION_ID" \
  --agent codex --workspace project --cwd /work/project --file ./next-task.md
```

A sandboxed coding agent often cannot reach the network, so `git fetch`, `git push`, and `gh` fail
even though the agent legitimately needs them to publish its work. The Herdr terminal server runs
outside that confinement, so a shell in one of its panes is not confined either. `herdr-run` turns
that into one narrow, audited door instead of an ad-hoc pile of keystroke injection.

> **This tool deliberately uses an out-of-sandbox channel.** Its allowlist prevents accidental and
> cooperative misuse; it is not a containment boundary against a hostile process running as the
> same user. Read the trust model in the user guide before widening policy.

## Installation

```sh
cargo install herdr-run
```

The crate builds on Rust 1.85 and newer and installs the `herdr-run` and `herdr-agent` binaries.

## What it does

- **Idempotent bring-up.** Finds or creates the Herdr workspace, the per-agent tab, and its pane.
  Running it twice does the setup work once; running it against half-built state completes only the
  missing part.
- **Allowlist by construction.** The command is split into argv, the program name is checked against
  a per-project allowlist, and every token is re-quoted before it reaches a shell - so shell
  metacharacters become literal arguments instead of a second command.
- **Real results, not screen scraping.** The pane redirects into a spool directory, preserving raw
  bytes and a genuine exit code. Raw mode passes those bytes through; JSON mode also includes
  base64 fields alongside readable UTF-8-with-replacement text.
- **Conservative readiness.** The foreground process group must prove the shell is idle twice; a
  prompt-tail check independently vetoes a half-typed human command when it has positive evidence.
- **Serialized account-wide use.** An account-global resolution lock prevents duplicate first-run
  workspaces/tabs, and an account-global per-pane lock spans readiness, launch, and collection, so
  callers from different projects cannot inject into the same pane concurrently.
- **Bounded tab growth.** Every agent that runs a command leaves a tab behind and nothing closes
  it, so the workspace grows until the Herdr server is the bottleneck (measured: 260 panes, >1000%
  CPU, every control call timing out). `max_panes` refuses to open a NEW tab past a ceiling -- never
  an existing one -- and `herdr-run reap` reports which tabs are provably finished with.
- **Visible, best-effort audit.** Refusals, admissions, failures, and completions are appended to a
  private JSONL log. Storage failures warn but never replace a completed command's exit status.

## Interactive-agent messaging

`herdr-agent` binds a private on-disk queue to one exact pane or stable agent session. It waits for
native `idle` or `done` state, durably marks the prompt in flight, submits the full literal text in
one operation, and requires a subsequent `working` event. A pre-submission failure stays pending
and safe to retry; an ambiguous post-submission failure is quarantined and is never automatically
submitted twice. `status` and `read` inspect a validated target without altering its queue.

## Configuration

Policy is per project, in the nearest `.herdr-run.yaml`. Every field has a working default, so a
project can adopt the tool with no configuration at all:

```yaml
workspace: agent-cmds     # Herdr workspace holding this project's command tabs
tab_name: "{agent}"       # one tab per agent
allow: [git, gh]          # cooperative policy rail - keep it small
prefixes: [with-proxy]    # wrappers that may precede an allowlisted program
spool_dir: .herdr-run     # git-ignored: holds command output, not source
max_panes: 32             # refuse a NEW tab once the workspace holds this many panes (0 = off)
```

## Quick reference

```
herdr-run <agent> '<command>'    run the command; exits with the command's own exit code
herdr-run check '<command>'      policy only: allowed or refused. Touches no pane.
herdr-run target                 resolve/bring up the pane; print ids and readiness
herdr-run config                 print the effective configuration
herdr-run reap                   report which command tabs are provably finished. Closes nothing.
herdr-run doctor                 verify the sandbox-crossing actually works, both directions
herdr-run userguide              the full user guide
herdr-agent send ...             durably submit one prompt to an interactive agent
herdr-agent drain ...            resume a bound queue without duplicating ambiguous prompts
herdr-agent status ...           inspect validated agent identity and queue state
herdr-agent read ...             read recent validated agent output
herdr-agent userguide            the complete messaging and recovery contract
```

Requires Linux with a working systemd user manager and a separately installed
[`herdr`](https://github.com/herdrdev/herdr) command in a fixed location such as `/usr/local/bin`,
`~/.local/bin`, or `~/bin`. The integration is tested with Herdr 0.8.0; compatible
newer releases must provide its `status`, `workspace`, `tab`, and `pane` command APIs.

Full documentation: `herdr-run userguide` and `herdr-agent userguide`.
