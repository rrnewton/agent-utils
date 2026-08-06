# herdr-run — user guide

Run an **allowlisted** command in a Herdr pane that lives **outside** the agent sandbox, and get its
real stdout, stderr, and exit code back.

```
herdr-run release-agent 'with-proxy git ls-remote origin main'
```

> **This tool deliberately crosses a security boundary.** It exists because an agent's confinement
> blocks `git`/`gh` network access that the agent legitimately needs in order to land work. Keep the
> allowlist as small as the project can tolerate, and read the threat model below before widening it.

---

## Why it exists

An agent process runs confined (BpfJailer plus a per-destination forward-proxy allowlist). Inside
that confinement:

```
$ with-proxy git ls-remote https://github.com/example-org/example-repo main
fatal: unable to access 'https://github.com/example-org/example-repo/': CONNECT tunnel failed, response 403
```

The Herdr terminal server runs outside the confinement, so shells in its panes are not confined. The
same command through a pane returns the real SHA. `herdr-run` turns that into one narrow, audited
door instead of an ad-hoc pile of `send-keys` calls.

---

## Usage

```
herdr-run <agent> '<command>'      # explicit agent -> tab name
herdr-run '<command>'              # agent taken from $DG_AGENT_NAME / $HERDR_RUN_AGENT
herdr-run check '<command>'        # policy only: allowed or refused. Touches no pane.
herdr-run target                   # resolve/bring up the pane; print ids and readiness
herdr-run config                   # print the effective configuration
herdr-run doctor                   # bracket the premise in both directions (see below)
herdr-run userguide                # this document
```

Useful flags:

| Flag | Effect |
| --- | --- |
| `--dry-run` | Admit the command and print the exact rendered line; execute nothing. |
| `--wait-ready S` | Wait up to `S` seconds for the pane to go idle instead of refusing at once. |
| `--timeout S` | Override the command timeout. |
| `--json` | One JSON object with `exit_code`, `stdout`, `stderr`, `run_id`, `pane_id`, spool paths. |
| `--cwd PATH` | Working directory for the command. |
| `--no-cache` | Ignore the session cache and re-resolve the pane from labels. |
| `--config PATH` | Use an explicit config file. |

By default stdout and stderr are passed straight through and **the process exits with the command's
own exit code**, so `herdr-run` composes in a shell script like the command it wraps.

### Exit codes

`0`-`255` from the wrapped command are passed through unchanged. herdr-run's own failures use
distinct codes:

| Code | Meaning |
| --- | --- |
| `77` | **REFUSED** by the allowlist. Nothing was executed. |
| `75` | **PANE BUSY** — the pane was not observably idle. Nothing was executed; retrying is meaningful. |
| `69` | Herdr server / workspace / tab / pane could not be established. |
| `76` | The command was launched but did not finish before the timeout. It is **still running**. |
| `78` | The project config is malformed. |

---

## Configuration — `.herdr-run.yaml`

The tool is generic; the policy is per-project. The nearest `.herdr-run.yaml` (or `.herdr-run.yml`)
searched from the working directory upward wins, `.gitignore`-style. With no config file at all the
built-in defaults apply, so a project can adopt the tool with zero configuration.

```yaml
# Herdr workspace LABEL holding this project's command tabs.
workspace: agent-cmds

# Tab-label schema. {agent} = invoking agent, {project} = project directory basename.
tab_name: "{agent}"

# Working directory for commands (relative paths resolve against the project root).
cwd: null

# THE ALLOWLIST. Programs that may run. Keep it small.
allow:
  - git
  - gh

# Wrapper programs that may PRECEDE an allowlisted program, never run on their own.
prefixes:
  - with-proxy

# Defense in depth (see Threat model) — not the boundary.
deny_global:
  git: ["-c", "--config-env", "--exec-path", "--namespace"]
deny_subcommand:
  git: ["filter-branch", "daemon", "instaweb"]
  gh:  ["alias", "extension", "ext", "codespace", "cs"]
deny_anywhere: ["--upload-pack", "--receive-pack"]

# Git-IGNORED directory for run spools and the audit log.
spool_dir: .herdr-run

timeout_seconds: 900          # wait for the command's exit code
ready_timeout_seconds: 0      # wait for the pane to go idle (0 = refuse immediately)

readiness: both               # 'both' = process signal AND prompt veto; 'process' = drop the veto
prompt_tail: null             # e.g. "$ ". null = infer from ~/.bashrc / ~/.zshrc, abstain on failure

broker: direct                # 'direct' | 'systemd-run' (see Brokering)
```

Unknown keys are a hard error. A typo'd `allowlist:` silently falling back to the default allowlist
is exactly the quiet policy failure this tool must not have.

---

## How it works

### Idempotent bring-up

Server → workspace → tab → pane, each resolved by looking for it before creating it:

1. **Server** — if `herdr status` says no server is running, start one. **Always** through
   `systemd-run --user`.
2. **Workspace** — find by label (default `agent-cmds`), else create it. A new workspace arrives with
   one default tab, which is *renamed* into the tab schema rather than having a second tab added.
3. **Tab** — find by label (default: the agent name), else create it.
4. **Pane** — the tab's single pane. A split tab is refused as ambiguous rather than guessed at.

Resolved ids are cached in `<spool_dir>/session-cache.json`, and the cache is **always re-validated**
against the live session (pane still exists; still belongs to a tab with the expected label) before
use. Any mismatch discards it and re-resolves from labels. Running the tool twice does the bring-up
work once; running it against half-built state completes only the missing part.

### Why the server must start via `systemd-run`

Panes are children of the Herdr **server**. If a confined agent starts the server, every pane it
creates inherits that confinement, and the tool silently reproduces the very `403` it exists to
avoid — nothing about such a pane looks different from a good one. `systemd-run --user` reparents the
server onto the user manager, outside the jail.

If a bad server is already running, `herdr-run doctor` will catch it. Its fix is
`herdr server stop`, then let `herdr-run` restart it.

### Readiness detection

Typing into a terminal someone else may be using is the dangerous part, so two independent signals
must agree and the check fails closed.

**Primary — foreground process group (an observable).** `herdr pane process-info` reports
`shell_pid` and `foreground_process_group_id`. A shell at its prompt owns its own foreground group;
when anything runs, the kernel moves that group to the job. This dereferences the running process
rather than inferring from pixels, so prompt themes, colour, and shell upgrades do not affect it.
Required for **two consecutive** polls, because a just-submitted line has a brief window before the
pgid moves.

**Secondary — prompt tail (a veto only).** The process signal cannot see a command a human *typed*
but has not run: the shell is genuinely idle, yet typing would concatenate onto their line. So the
last rendered line is checked against the prompt's trailing literal, inferred from `PS1` in the shell
rc file (a `PS1` ending `...\n\$ ` yields `"$ "`):

- line ends with the tail → `clean`
- tail appears earlier in the line → `dirty` → **refuse**
- tail not found, or not inferable → `abstain`

`abstain` never upgrades a pane to ready — the veto only fires on positive evidence of dirt, and says
so when it does not understand the prompt. Set `prompt_tail` explicitly for an exotic prompt, or
`readiness: process` to drop the veto entirely.

Both verdicts are recorded in each run's `meta.json`, so a run never implies a check that did not run.

### Result capture — files, not screen scraping

The pane executes:

```sh
{ cd <cwd> && <command> ; } >stdout 2>stderr; printf '%s\n' "$?" >exit_code
```

into `<spool_dir>/runs/<run_id>/`, and the caller reads those files directly — the filesystem is
shared across the sandbox boundary. Scraping the terminal would inherit hard-wrapping mid-token,
silent scrollback truncation, ANSI/progress-bar corruption, and — decisively — **no exit code
anywhere on screen**. The appearance of `exit_code` is both the completion signal and the result; a
value that does not parse as an integer is treated as "still being written", not as corrupt.

Each run directory holds `command`, `stdout`, `stderr`, `exit_code`, and `meta.json`.

### Audit log

Every attempt appends one JSON line to `<spool_dir>/audit.jsonl` — including **refusals**, which is
the run of events most worth noticing. A log recording only successes would make the allowlist's
behaviour unobservable after the fact.

### Brokering

Herdr control calls default to `direct`: measured on `devbig014` 2026-08-06, `herdr status --json`
returns identical JSON from inside the jail and through a `systemd-run` wrapper, because the server's
unix socket is reachable. Set `broker: systemd-run` on a host where it is not. This setting does
**not** affect server startup, which always uses `systemd-run` (above).

---

## Threat model — read before widening `allow`

**What is actually guaranteed:** the command is split with `shlex`, the program name is checked
against the allowlist, and **every token is re-quoted with `shlex.quote`** before being embedded in
the shell wrapper. So the pane executes exactly the argv that was admitted. There is no
metacharacter blocklist, deliberately: a list of `;`, `&&`, backticks, `$()`, newlines… is a *proxy*
for "cannot start a second command", whereas re-quoting is the property itself.

Also enforced: the program must be a **bare name** resolved from the pane's `PATH` (so `./git` or
`/tmp/gh` cannot masquerade), and each wrapper prefix may appear at most once.

**What is NOT guaranteed:** anything an allowlisted program can do by itself. `git` writes files,
runs hooks from the repository being operated on, and honours ambient configuration. `gh`
authenticates as you and can modify repositories. The `deny_*` lists remove the best-known
self-escapes (`git -c alias.x='!sh'`, `git --exec-path=…`, `gh extension exec`) but they are
**defense in depth, not the boundary** — treat "an agent can run `git`" as the actual privilege being
granted, and size the allowlist accordingly.

**Operational notes:**

- On timeout the command is **not** killed. It runs in a terminal this process does not own, and
  signalling it blind could hit whatever the pane is doing by then. The error names the pane and the
  spool directory so it can be inspected.
- `spool_dir` holds real command output. It must be git-ignored.
- The panes are shared with a human. Bring-up always uses `--no-focus`, and readiness refuses rather
  than typing over someone.

---

## Verifying it works

```
herdr-run doctor
```

Brackets the premise in **both** directions rather than asserting it: the probe command must **fail**
in-jail and **succeed** through the pane. Three verdicts:

- blocked in-jail, succeeds via pane → working as intended;
- succeeds both ways → the pane is buying nothing here, run the command directly;
- fails via pane → the path is broken, most likely a server started from inside a sandbox.
