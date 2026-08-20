# herdr-run — user guide

Run an **allowlisted** command in a Herdr pane that lives **outside** the agent sandbox, and get its
real stdout, stderr, and exit code back.

```
herdr-run release-agent 'with-proxy git ls-remote origin main'
```

> **This tool deliberately uses an out-of-sandbox channel.** It exists because an agent's
> confinement can block network access needed to land work. Its allowlist is a cooperative safety
> rail, not a containment boundary against a hostile same-user process. Read the trust model below.

## Installation

```sh
cargo install herdr-run
```

Rust 1.85 or newer is required to build from source. The installed binary is self-contained.

### Prerequisites

`herdr-run` requires Linux with a working systemd user manager and a separately installed
[`herdr`](https://github.com/herdrdev/herdr) command. The integration is tested with Herdr 0.8.0;
compatible newer releases must provide its `status`, `workspace`, `tab`, and `pane` command APIs.

---

## Why it exists

An agent process may run inside a sandbox whose network policy blocks a destination it legitimately
needs in order to publish work. Inside that confinement:

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
herdr-run reap                     # report which command tabs are provably finished. Closes nothing.
herdr-run doctor                   # bracket the premise in both directions (see below)
herdr-run userguide                # this document
```

Useful flags:

| Flag | Effect |
| --- | --- |
| `--dry-run` | Admit the command and print the exact rendered line; execute nothing. |
| `--wait-ready S` | Wait up to `S` seconds for the pane to go idle instead of refusing at once. |
| `--timeout S` | Override the command timeout. |
| `--json` | One JSON object with exit status, readable text, byte-exact base64 streams, target, and spool paths. |
| `--cwd PATH` | Working directory for the command. |
| `--no-cache` | Ignore the session cache and re-resolve the pane from labels. |
| `--config PATH` | Use an explicit config file. |

By default stdout and stderr are passed straight through and **the process exits with the command's
own exit code**, so `herdr-run` composes in a shell script like the command it wraps.

### Exit codes

Wrapped-command statuses are passed through unchanged. Wrapper failures use these stable codes:

| Code | Meaning |
| --- | --- |
| `77` | **REFUSED** by the allowlist. Nothing was executed. |
| `75` | **PANE BUSY** — the pane was not observably idle. Nothing was executed; retrying is meaningful. |
| `69` | Herdr server / workspace / tab / pane could not be established. **Not a retry signal**: it covers both a transient bring-up failure and the `max_panes` refusal, and the second only clears when somebody closes tabs. `75` is the only code that promises retrying is meaningful. |
| `76` | The command was launched but did not finish before the timeout. It is **still running**. |
| `78` | The project config is malformed. |

A wrapped command can itself return one of those numbers, so the number alone is ambiguous in raw
mode. With `--json`, a completed wrapped command emits a result object (even when nonzero), while a
wrapper failure emits no result object and writes a `herdr-run:` diagnostic to stderr.

Timeout values must be finite, non-negative, and no greater than 31,536,000 seconds (one year).
That shared upper bound prevents a very large finite value from overflowing a platform clock into
an accidental unbounded wait.

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

# Working directory for commands. null = caller's cwd; relative paths use the project root.
cwd: null

# Cooperative allowlist. Programs that may run. Keep it small.
allow:
  - git
  - gh

# Optional Cargo trust widening: add `cargo` to `allow` only if the project accepts that even
# dependency-oriented Cargo commands may execute ambient wrappers/helpers. This fail-closed list
# then prevents compilation-oriented and unknown subcommands; see Threat model.
allow_subcommand:
  cargo: [fetch, update, generate-lockfile, vendor, metadata, tree, search]

# Wrapper programs that may PRECEDE an allowlisted program, never run on their own.
prefixes:
  - with-proxy

# Defense in depth (see Threat model) — not the boundary.
deny_global:
  git: ["-c", "--config-env", "--exec-path", "--namespace"]
  # Cargo accepts these before or after its subcommand; attached forms are also refused.
  cargo: ["--config", "-Z"]
deny_subcommand:
  git: ["filter-branch", "daemon", "instaweb"]
  gh:  ["alias", "extension", "ext", "codespace", "cs"]
deny_anywhere: ["--upload-pack", "--receive-pack"]

# Git-IGNORED directory for run spools and the audit log.
spool_dir: .herdr-run

timeout_seconds: 900          # wait for the command's exit code
retention_days: 4             # completed run spools older than this are pruned on a later write
max_panes: 64                 # refuse to open a NEW tab once the workspace holds this many panes
ready_timeout_seconds: 0      # wait for the pane to go idle (0 = refuse immediately)

readiness: both               # 'both' = process signal AND prompt veto; 'process' = drop the veto
prompt_tail: null             # e.g. "$ ". null = infer from ~/.bashrc / ~/.zshrc, abstain on failure

broker: direct                # 'direct' | 'systemd-run' (see Brokering)
```

Unknown or duplicate keys, merge keys, non-finite/excessive timeouts, fractional/negative retention
days, retention beyond 365,000 days, a fractional/negative/oversized `max_panes`, control
characters, and unsupported tab placeholders are hard errors. A typo'd policy key must not silently
fall back to defaults.

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
against the live session (unique workspace label and id; unique tab label and id; pane, tab, and
workspace relationship) before use. Any mismatch discards it and re-resolves from labels. An
account-global resolution lock covers lookup, creation, and cache publication, so simultaneous
first runs from different projects cannot create duplicate labels. Running the tool twice does the
bring-up work once; running it against half-built state completes only the missing part.

### Why the server must start via `systemd-run`

Panes are children of the Herdr **server**. If a confined agent starts the server, every pane it
creates inherits that confinement, and the tool silently reproduces the very `403` it exists to
avoid — nothing about such a pane looks different from a good one. `systemd-run --user` reparents the
server onto the user manager, outside the jail.

The launcher ignores caller-provided `HOME` and `PATH` for brokered execution. It reads the account
home from the user database, resolves `herdr` from a fixed set of install locations, canonicalizes
the result, and passes that absolute path to systemd. This blocks a planted workspace executable;
it cannot make an owner-writable per-user install trustworthy against another same-user process.

If a bad server is already running, `herdr-run doctor` will catch it. Its fix is
`herdr server stop`, then let `herdr-run` restart it.

### Readiness detection

Typing into a terminal someone else may be using is the dangerous part. The foreground-process
signal is authoritative and must prove idleness twice. The independent prompt signal can veto on
positive evidence of a half-typed line; when no prompt tail can be inferred it records `abstain`
instead of pretending a check succeeded.

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

`abstain` does not veto an otherwise idle pane. Set `prompt_tail` explicitly for an exotic prompt,
or `readiness: process` to disable this secondary check.

Both verdicts are recorded in each run's `meta.json`, so a run never implies a check that did not run.

### Result capture — files, not screen scraping

The pane invokes a POSIX shell with this inner wrapper, so configured interactive shells such as
Fish do not have to parse POSIX grouping syntax themselves:

```sh
{ cd <cwd> && <command> ; } >stdout 2>stderr; printf '%s\n' "$?" >exit_code
```

into a newly and exclusively created `<spool_dir>/runs/<run_id>/`, and the caller reads those files
directly. Scraping the terminal would inherit hard-wrapping mid-token,
silent scrollback truncation, ANSI/progress-bar corruption, and — decisively — **no exit code
anywhere on screen**. The appearance of `exit_code` is both the completion signal and the result; a
value that does not parse as an integer is treated as "still being written", not as corrupt.

Raw mode writes `stdout` and `stderr` back without decoding. JSON mode includes `stdout_base64` and
`stderr_base64` for byte-exact consumers and readable `stdout`/`stderr` decoded as UTF-8 with invalid
sequences replaced.

Each run directory holds `command`, `stdout`, `stderr`, `exit_code`, and `meta.json`. Directories the
tool creates for spool, run, cache, and account-global lock state use mode `0700`; command output,
metadata, cache, and lock files use `0600`, independent of the caller's umask. A pre-existing
configured spool directory is never chmoded: only tool-owned children and files are created below
it, so `spool_dir: .` cannot silently change a project directory's permissions.

If writing `meta.json` fails after a command completes, the failure is warned and recorded when
possible, `--json` reports `"meta": null`, and the command's output and exit status are still
returned.

### Retention

Run spools are pruned when a **new run is written**, not by a timer: a scheduled job that silently
stops is indistinguishable from one that runs and finds nothing to do. A run is eligible only after
it has a regular, parseable `exit_code` completion marker, and its age is measured from that file's
mtime. Completed runs older than `retention_days` (four by default) are removed; active, timed-out,
or incomplete runs are retained even if their directory itself is old.

Scope is enforced rather than trusted: a symlink in the configured root or any ancestor disables
that prune pass; entry symlinks are skipped; only directories whose lexical and canonical parent is
the exact runs root are considered; and the root itself is never removed. The **audit log is never
pruned**: it is a separate best-effort evidence trail, and a record that deletes itself would be
misleading.

### The pane cap and `reap`

Every agent that ever runs a command leaves a tab behind, and **nothing closes it**. Agents are
coined and destroyed continuously, so the command workspace grows for as long as agents are coined
— until the Herdr server itself becomes the bottleneck. That end state is measured, not feared: on
a 316-core host a session with 260 panes drove the server to over 1000% CPU, with roughly 98% of
cycles in the allocator's futex spinlock and every control call timing out.

`max_panes` (64 by default) turns that slow collapse into one legible refusal. When the workspace
already holds that many panes and this agent has **no tab yet**, bring-up fails with exit 69 and a
message naming the remedy. The check is deliberately only on the create path: an agent whose tab
already exists is never locked out of it, because a cap that can break work in progress is a cap
that gets switched off. Set `max_panes: 0` to disable it.

The cap is **per workspace**, while the measurement behind the number is **per server**. Five
projects each with their own `workspace:` and each sitting at 63 panes reproduce the measured
condition without any of them breaching its cap. `max_panes` bounds one project's contribution to
the leak, not a host's total.

`herdr-run reap` says which tabs can go. It considers only panes **this tool has run a command in**
(read from the run spool, not from `herdr pane list`, so a human's tab in the same workspace is not
a candidate), and it re-derives scope from the current configuration: the recorded workspace label
must equal `workspace`, and the recorded tab label must equal what `tab_name` renders today for the
recorded agent. Retarget either and the old tabs immediately fall out of scope.

**The candidate set is bounded by retention, and this matters.** Run records are the candidates,
and herdr-run deletes a run record `retention_days` (four by default) after the run finished. A tab
whose owning agent last ran a week ago therefore has no surviving record, is not a candidate, and
will never appear in this report — while still holding a pane and still counting against
`max_panes`. The oldest leaks are the ones `reap` cannot see, and they have to be closed by hand.
The report prints `candidate_source.retention_days` for exactly this reason: `"considered": 3` means
three panes were *eligible to look at*, not that the workspace holds three tabs.

A tab is reported STALE only when all three hold, each of them positive evidence:

1. **No in-flight work** — every run naming the pane recorded an `exit_code`. A run without one is
   the agent-is-thinking case, and it wins over every other signal. That state is written when a
   command outlives its timeout: it is still running in a pane nobody owns, so the run is recorded
   with a null exit code rather than not recorded at all. If the command later finishes, its
   `exit_code` file is re-read on the next sweep, so one timeout cannot make a pane permanently
   unreapable.
2. **The pane's shell is gone** — herdr still lists the pane, but `/proc` says the shell pid it
   reports does not exist. Note this is the PANE shell, not the trailing PID in the run directory
   name: that one is the short-lived `herdr-run` CLI and is dead for every completed run, so a
   policy anchored on it would classify every healthy idle agent as stale.
3. **Reuse and reboot are excluded** — identity is the triple `(pid, boot_id, start_ticks)` from
   field 22 of `/proc/<pid>/stat`, recorded in `meta.json` at run time and compared against the live
   process. A recycled pid, a new pane incarnation, or a record from a previous boot is UNKNOWN.

Everything else — an unreadable or unparseable `/proc` entry, a pane herdr no longer lists, a
workspace label herdr cannot resolve, a control call that fails — is UNKNOWN, and UNKNOWN is never
reaped. Only a `/proc` entry that is *positively absent* may contribute to STALE; "could not tell"
never does. **`reap` closes nothing**; it prints the plan, with a reason
for every pane it declined and a count for every verdict including the zeros, because "reaped 0
because nothing was stale" and "reaped 0 because the detector is inert" are otherwise the same
output.

### Audit log

`herdr-run` attempts to append JSON lines to `<spool_dir>/audit.jsonl` for **refusals**, dry runs,
admission before target resolution/launch, wrapper failures, and completed commands. The `doctor`
pane probe follows the same path. This makes a successful run a two-phase record (`ADMITTED`, then
`RAN`) and leaves an admission marker when a later control operation fails.

The audit is deliberately best effort: an append failure produces a warning but cannot mask a
completed command's output or exit status. The file is private (`0600`) and each line is issued as
one append write, but it is not fsynced, remotely replicated, immutable, or protected from another
same-UID process. It is operational evidence, not durable or tamper-proof security logging.

### Brokering

Herdr control calls default to `direct`, which works when the server's Unix socket is reachable from
the caller. Set `broker: systemd-run` on a host where it is not. This setting does **not** affect
server startup, which always uses `systemd-run` (above).

---

## Trust and safety model — read before widening `allow`

**What the wrapper guarantees for cooperative callers:** the command is split with POSIX shell-word
rules, the program name is checked against policy, terminal control characters are rejected, and
every token is safely re-quoted before it is embedded in the shell wrapper. So the pane executes
the admitted argument vector. There is no
metacharacter blocklist, deliberately: a list of `;`, `&&`, backticks, `$()`, newlines… is a *proxy*
for "cannot start a second command", whereas re-quoting is the property itself.

Also enforced: the program must be a **bare name** resolved from the pane's `PATH` (so `./git` or
`/tmp/gh` cannot masquerade), and each wrapper prefix may appear at most once.

**`cargo` is a special case and is not allowed by default.** Even `fetch` and `metadata` can execute
ambiently configured compiler wrappers, credential providers, or fetch helpers, so calling those
commands "download only" would overstate the boundary. A project may explicitly add `cargo` to
`allow` when it accepts that trust widening. The built-in `allow_subcommand` policy then limits the
opt-in to dependency-oriented commands; `build`, `test`, `run`, `install`, `clippy` and every
third-party `cargo-*` subcommand remain refused. `--config` and unstable `-Z` flags are refused in
every argument position, including attached forms. Fetching outside and building in-jail with
`--offline` can still be operationally useful, but it is not a no-code-execution guarantee.

**What is NOT guaranteed:** anything an allowlisted program can do by itself. `git` writes files,
runs hooks from the repository being operated on, and honours ambient configuration. `gh`
authenticates as you and can modify repositories. The `deny_*` lists remove the best-known
self-escapes (`git -c alias.x='!sh'`, `git --exec-path=…`, `gh extension exec`) but they are
**defense in depth, not the boundary** — treat "an agent can run `git`" as the actual privilege being
granted, and size the allowlist accordingly.

This process runs under the same user identity as the caller. If that caller can access Herdr's
socket or the user-systemd bus directly, it can bypass this command, its project policy, and its
audit. An enforceable security boundary requires an out-of-jail broker that owns immutable policy
and audit storage, plus sandbox rules denying direct Herdr and systemd access. This package does not
claim to provide that stronger architecture.

**Operational notes:**

- On timeout the command is **not** killed. It runs in a terminal this process does not own, and
  signalling it blind could hit whatever the pane is doing by then. The error names the pane and the
  spool directory so it can be inspected.
- `spool_dir` holds real command output. It must be git-ignored.
- The panes are shared with a human. Bring-up always uses `--no-focus`, and readiness refuses rather
  than typing over someone.
- An account-global per-pane file lock serializes concurrent callers across projects and processes.
  It cannot eliminate the inherent race with a human typing immediately after the final readiness
  sample.
- Each Herdr/systemd control subprocess is bounded to 30 seconds and killed/reaped on expiry. That
  control bound is separate from the configured timeout of the command running in the pane.

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
