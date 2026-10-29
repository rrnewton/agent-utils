# Agent harness environment and PATH

Coding agents run their tool commands in shells the harness starts for them. Which `agentctl`,
`wrkslots` or `dagrun` such a shell finds depends on how the harness builds that shell's
environment. It is easy to get wrong: a directory you put first on `PATH` before launch can end up
behind `~/bin` by the time a tool command runs. This page records what reaches an agent's tool
commands, at launch and mid-session, for the harnesses and terminal hosts used with this suite,
and gives tested recipes for putting a team's own tool directory first.

Tested on 2026-10-29 with Claude Code 2.1.310, Codex CLI 0.163.0, Muse Code 1.4.15, Herdr 0.8.0,
tmux 3.7b and bash 5. Harness internals change between releases; re-run [the verification
recipe](#verify-any-harness) after an upgrade, and for any harness not listed here.

## Summary

| | Claude Code | Codex | Muse |
|---|---|---|---|
| How a tool command runs | a fresh `bash -c` that sources a shell snapshot taken at start | a fresh `bash -c` that gets its environment from a **login**-shell snapshot taken at start | a fresh `bash -c` with the harness's own environment |
| `PATH` exported before launch reaches tool commands | yes: the snapshot writes the launch `PATH` back verbatim | **no**: the login snapshot runs `~/.bash_profile`, which can put other directories first | yes |
| `export` in one tool call is seen by the next | no | no | no |
| `BASH_ENV=<file>` exported at launch | sourced before every command, so edits apply on the next call | same | same |
| Harness-specific per-command env file | `CLAUDE_ENV_FILE` and hook-written env files (cached, see below) | its own snapshot file (undocumented) | none found |
| Change `PATH` of an agent that is already running | yes, through hooks in project settings (needs one `cd`) | only by editing its snapshot file (undocumented) | no; relaunch it with resume |

Any of them, launched with `BASH_ENV` (or another per-command file that sources a team file), picks
up an edit of that team file on its next tool call. The last row is about agents that were started
without one.

| Terminal host | `PATH` given when the pane is created |
|---|---|
| Herdr (`herdr tab create --env PATH=...`) | reaches the pane shell, but an interactive shell then runs `~/.bashrc`, which may put other directories first |
| tmux (`new-window -e PATH=...`) | **replaced** by the tmux server's global `PATH` when the pane spawns, even when the window runs a program directly with no shell |

`agentctl start` passes profile `env` entries and `--env KEY=VALUE` to the terminal host this way,
then types the harness command line into the pane's interactive shell. A `PATH` entry in a profile
therefore loses to the shell's rc files on Herdr, and to tmux itself on tmux. Put `PATH` on the
harness command line instead (see [Recipes](#recipes)).

## How each harness builds a tool command's environment

### Claude Code

Each Bash tool call runs `/bin/bash -c 'source <snapshot>; <session env>; eval <command>'` in a new
process. The snapshot (`~/.claude/shell-snapshots/snapshot-bash-*.sh`) is written once, when the
session starts. It holds functions, aliases and options from your shell startup files, and it ends
with `export PATH=<PATH of the Claude Code process>`. So:

- A `PATH` exported before launch reaches every tool command unchanged.
- `export PATH=...` inside a tool call lasts only for that call. `cd` does persist; Claude Code
  moves the shell back into the project directory when it leaves it.
- `PATH` in settings `env` (for example `.claude/settings.local.json`) sets the process `PATH` at
  start. It is a literal value: `$PATH` is not expanded. A mid-session edit of settings `env`
  applies at once to other variables, but **not** to `PATH`, because the snapshot already fixed it.

The session env script is the supported way to change the environment of later calls. Claude Code
sources, before every Bash command:

1. the file named by `CLAUDE_ENV_FILE` in its own environment at launch; and
2. the env files that hooks wrote: every `SessionStart`, `CwdChanged` and `FileChanged` hook runs
   with `CLAUDE_ENV_FILE` pointing at a per-hook file, and the export lines it appends there apply
   to every later Bash command.

Details that matter in practice:

- The combined script is **cached**. An edit to the launch `CLAUDE_ENV_FILE` takes effect at the
  next cwd change, not on the next call.
- A cwd change clears the files written by earlier `CwdChanged` and `FileChanged` hooks, so a
  `CwdChanged` hook must rewrite its exports every time it runs.
- `FileChanged` watches the file names its matcher lists in the current directory, plus any
  absolute paths a `CwdChanged` or `FileChanged` hook returns as
  `hookSpecificOutput.watchPaths`. A `FileChanged` hook present at launch applies an edit to the
  watched file on the very next Bash call, with no action by the agent.
- Hooks and settings files are re-read while the session runs. A `FileChanged` hook added
  mid-session does not arm by itself; it arms once a `CwdChanged` hook returns `watchPaths`.
- Every hook env file is sourced, so two writers that copy different contents stack
  (`teamC:teamA:...`). Have hooks write a source line for one team file (see
  [Recipes](#recipes)) and make that file idempotent.

### Codex

Each shell tool call runs `/bin/bash -c <command>` in a new process. Its environment comes from a
snapshot of a **login** shell that Codex takes at session start
(`~/.codex/shell_snapshots/<thread>.<nanos>.sh`, the `shell_snapshot` feature). Because the login
shell runs `~/.bash_profile`, a startup file that prepends `~/bin` (or anything else) puts that
directory ahead of a `PATH` prefix you exported before launch. To keep the prefix:

- `-c allow_login_shell=false` makes the snapshot use a non-login shell, so the launch `PATH`
  keeps its order; or
- `-c 'shell_environment_policy.set.PATH="<full PATH>"'` sets a literal `PATH` (no `$PATH`
  expansion) that wins over the startup files.

`shell_environment_policy` also offers `inherit`, `exclude`, `include_only` and
`ignore_default_excludes`. A mid-session edit of the project's `.codex/config.toml` is not picked
up. A running Codex sources its own snapshot file before every command. Appending the line
`. /path/to/team-env.sh` to that file therefore changes `PATH` on the next call, and later edits of
the team file apply too. Codex still puts its own bundled-tools directory first. The snapshot file
is internal and undocumented: treat editing it as a last resort, and prefer relaunching.

### Muse

Each shell tool call runs a fresh non-interactive `bash -c` whose environment is the Muse
process's own; no snapshot or login shell is involved. So:

- A `PATH` exported before launch reaches every tool command unchanged.
- `export` inside a tool call lasts only for that call.
- `BASH_ENV` exported at launch is sourced before every command; edits apply on the next call.
- No per-command env file or hook that changes the environment of later calls was found, and the
  environment of a running Muse cannot be changed from outside. To give a running agent a new
  `PATH`, relaunch it with resume (`muse resume <session>`, or `agentctl revive`) with the right
  launch environment.

### Other `bash -c` harnesses

Any harness that runs each command as a non-interactive `bash -c` with its own environment
behaves like Muse: it keeps a `PATH` exported before launch and forgets an `export` made inside a
command. Bash sources `$BASH_ENV` at the start of every non-interactive shell, which makes it the
portable per-command env file.

### Shell startup files: the usual trap

Interactive shells read `~/.bashrc`; login shells read `~/.bash_profile` (or `~/.profile`). A line
such as

```sh
case "$PATH" in "$HOME/bin"|"$HOME/bin":*) ;; *) export PATH="$HOME/bin:$PATH" ;; esac
```

puts `~/bin` first again whenever anything else is first. It undoes a team prefix in every shell
that reads the file: a Herdr or tmux pane's interactive shell, and Codex's login snapshot. Claude
Code's snapshot is not affected, because it writes the process `PATH` back afterwards. Non-login
`bash -c` shells read neither file.

## Recipes

Let `TEAM_BIN` be the team's own tool directory (for example `<checkout>/common/bin`, or a
directory of links into it), and `TEAM_ENV` a small file the team owns:

```sh
# TEAM_ENV: idempotent, safe to source any number of times
TEAM_BIN=/path/to/team/bin
PATH="$TEAM_BIN:${PATH//$TEAM_BIN:/}"
export PATH
```

### Put the team directory first for a new agent

Prefix the harness command itself, so the prefix applies after the pane shell's startup files have
run, on any terminal host:

```sh
env BASH_ENV="$TEAM_ENV" PATH="$TEAM_BIN:$PATH" claude ...
env BASH_ENV="$TEAM_ENV" PATH="$TEAM_BIN:$PATH" codex -c allow_login_shell=false ...
```

`BASH_ENV` makes `TEAM_ENV` the source of truth for the whole session: edit it, and the next tool
call of every agent launched this way sees the change. This was tested with Claude Code, Codex and
Muse. In Codex, `BASH_ENV` runs after the login profile, so the prefix wins even
without `allow_login_shell=false`; the flag keeps the launch `PATH` order intact as well. For
Claude Code, a `CLAUDE_ENV_FILE` that holds only the line `. "$TEAM_ENV"` is an alternative (see
below).

### agentctl: one launch environment for a whole registry

`agentctl` does the above for every interactive agent a registry starts or revives when its
`.agentctl/profiles.json` has a top-level `launch_env`:

```json
{
  "schema": "agentctl-profiles/v1",
  "launch_env": {"env_file": "/path/to/team-env.sh", "path_prepend": ["/path/to/team/bin"]},
  "profiles": {}
}
```

Before the harness starts, `agentctl` types
`export BASH_ENV='<env_file>' PATH='<dirs>':"$PATH"` into the new pane's shell (inside the slot
box, for `--slot`), after that shell's startup files have run. The prefix therefore survives
`~/.bashrc` on Herdr and tmux alike, and `BASH_ENV` keeps later edits of the team file live. With
`path_prepend`, Codex also gets `-c allow_login_shell=false`. A start refuses when the file or a
directory is missing, and under root slot isolation. Tested end to end: on Herdr with Claude Code,
Codex and Muse, the team directories came first in each agent's tool calls. On tmux, with Claude
Code, they came first as well, and an edit of the env file applied on the agent's next call.

### Make later changes apply without a restart (Claude Code)

Write a **source line** into the hook's env file instead of copying the team file's contents.
Claude Code caches the text of its env files, but the line `. /path/to/team-env.sh` reads the team
file afresh in every Bash call. Edits then apply on the next call, with no `FileChanged` hook and
no stacking of old values:

```json
{"hooks": {"SessionStart": [{"hooks": [{"type": "command",
  "command": "echo '. /path/to/team-env.sh' >> \"$CLAUDE_ENV_FILE\""}]}]}}
```

The same indirection works for a launch-time `CLAUDE_ENV_FILE`: point it at a file whose only line
is `. /path/to/team-env.sh`, never at the team file itself.

### Change PATH of a Claude Code agent that is already running

1. Add a `CwdChanged` hook to that agent's project `.claude/settings.local.json`. Claude Code
   re-reads the file. Merge the hook into the settings already there; do not overwrite them.

   ```json
   {"hooks": {"CwdChanged": [{"hooks": [{"type": "command",
     "command": "echo '. /path/to/team-env.sh' > \"$CLAUDE_ENV_FILE\""}]}]}}
   ```

2. Ask the agent to run `cd /` once. Any directory change fires the hook.

From then on the agent's Bash calls see the team `PATH`, and every edit of the team env file
applies on its next call. The hook runs again on every later directory change, which is needed:
each change clears the env files of earlier `CwdChanged` hooks.

### Verify any harness

Use a throwaway agent and two fake tools; never test against real agents.

```sh
mkdir -p /tmp/envtest/team{A,B}/bin
for t in A B; do printf '#!/bin/sh\necho TEAM-%s\n' $t > /tmp/envtest/team$t/bin/agentctl
  chmod +x /tmp/envtest/team$t/bin/agentctl; done
cat > /tmp/envtest/probe <<'EOF'
#!/bin/sh
{ echo "== $1 agentctl=$(command -v agentctl) FOO=${FOO-unset}"
  echo "$PATH" | tr : '\n' | grep -n 'team\|/bin$' | head -5; } >> /tmp/envtest/log
EOF
chmod +x /tmp/envtest/probe
```

Launch the harness with `PATH=/tmp/envtest/teamA/bin:$PATH FOO=launch` (and, separately, with
`BASH_ENV` pointing at a file that prepends `teamB`). Ask it to run, as separate tool calls:

1. `/tmp/envtest/probe first`
2. `export PATH=/tmp/envtest/teamB/bin:$PATH FOO=changed; /tmp/envtest/probe same-call`
3. `/tmp/envtest/probe next-call`

Then edit the env file and ask for a fourth probe. The log answers each question in this page's
summary table for that harness: whether the launch prefix survived, whether an export persists,
and whether the per-command file is re-read.

## Which tool binary am I calling?

- `command -v agentctl` (or `type -a agentctl`) shows the first match on `PATH`; a directory such
  as `~/bin` placed first silently wins.
- The tracked launchers in `common/bin/` print one provenance line per call on stderr, for example
  `[agentctl] rust cache=verified source=<source fingerprint> binary=<binary sha256>`, so logs show
  which checkout and build ran.
- A copied binary carries no build commit today (`agentctl --version` prints the package version
  only). Compare `sha256sum` against the build you expect.
- A symlink to a launcher works from any directory, because the launchers resolve their own real
  path. The link must have the tool's own name: the launchers dispatch on `basename "$0"`, so
  `agentctl-dev -> common/bin/agentctl` is refused.
- The Rust launchers check source freshness on every call. A cached call measured about 0.45 s
  against a few milliseconds for the binary itself. The first call after any change under `rs/`
  rebuilds that tool (about 40 s on a loaded host), and concurrent calls queue on one checkout-wide
  lock. Weigh this when putting launchers on a busy agent's `PATH`.
