# gh-paced quickstart

**Problem.** The GitHub CLI (`gh`) has no client-side pacing. A script or an
agent that loops over `gh` can make dozens of calls a minute. If several hosts
share one GitHub account, their calls add up. GitHub answers with secondary rate
limits, and an account that keeps calling after that can be suspended. The only
related feature in `gh` itself is the response cache, `gh api --cache`.

**What gh-paced does.** It runs in front of the real `gh`. It classifies every
call, charges it against a per-host, per-account budget, and sleeps when a
budget is used up, printing a loud warning line on stderr. It also:

- reads GitHub's own account-wide numbers (`GET /rate_limit`) and slows down or
  stops as the remaining allowance shrinks;
- pauses every call for 15 minutes (or as long as `Retry-After` asks) after a
  rate-limit or abuse response. An HTTP 403 counts only when it is a rate
  limit: a permission denial (`Must have admin rights`, `Resource not
  accessible by integration`, ...) is reported and starts no cooldown, and any
  other 403 holds only its own call for 30 s, so a missing permission never
  stops other agents' calls or git's credential helper;
- refuses `gh api` calls to endpoints that need admin rights (webhooks,
  Actions permissions, ruleset and branch protection changes, ...) before they
  reach GitHub, unless `GH_PACED_ALLOW_ADMIN_API=1`;
- refuses write bodies larger than 8 KiB or containing long base64 runs,
  whether they come from an argument, a file, stdin, one of gh's ordinary
  aliases (which gh-paced expands itself, so gh never sees the alias name, as
  long as gh's `config.yml` is not rewritten while the call runs; a shell
  alias, `!...`, is passed to gh by name, only its arguments are checked, and
  what its shell command sends is checked only if the `gh` it runs is
  gh-paced), or
  the editor gh opens (gh-paced points gh's editor at a guard that checks the
  saved text);
- refuses writes whose body gh would compose from something it reads later
  (a template, `--fill`, the commit-message default of an interactive
  `pr create`): pass `--body` or `--body-file` instead.

Every process on the host shares the budgets through one locked state file. The
command's own output, exit status, stdin and terminal pass through unchanged.

**Dependencies.** Linux, the real `gh` binary, and a Rust toolchain to build.

## Install

Build it in the agent-utils checkout. agent-utils' own tools find this build
there, so no install step is needed for them:

```bash
cargo build --release --manifest-path rs/Cargo.toml -p gh-paced
```

To call it by name elsewhere, copy `rs/target/release/gh-paced` into any
directory on your `PATH`.

## How agent-utils tools choose their gh

Every agent-utils tool that calls GitHub (for example wrkviz's pull metadata,
dagrun's github-artifacts backend and pr-landing-planner) picks its command in
this order, first match wins:

1. `$AGENT_UTILS_GH`, an explicit command (for example `gh` to run unpaced, or
   your own wrapper);
2. gh-paced built in the agent-utils checkout the tool runs from
   (`rs/target/release/gh-paced` or `rs/target/<host>/release/gh-paced`), then
   a `gh-paced` on `PATH`. It wraps the real gh: `$GH_PACED_REAL_GH`, else the
   first `gh` on `PATH` that is not itself a gh-paced wrapper script. The
   account is `$GH_PACED_ACCOUNT`, else the real gh's signed-in login
   (`gh config get user -h github.com`); with neither, the tool refuses rather
   than run GitHub unpaced;
3. `gh` on `PATH`.

No home-directory location is assumed, and nothing is built on demand.

## Use

Call it directly, naming the account whose budgets apply:

```bash
gh-paced --account octocat -- pr view 12 --json state
```

Or make your own `gh` wrapper script hand every call to it:

```bash
exec gh-paced --account octocat --real-gh /usr/bin/gh -- "$@"
```

Check the budgets at any time. This reads local files only and never contacts
GitHub:

```bash
gh-paced status --account octocat
```

See how a command would be charged, without running it:

```bash
gh-paced classify -- api -X POST repos/o/r/issues/1/comments -f body=hi
```

## Default budgets

These are per host and per account, sized so that four hosts sharing one account
stay well under GitHub's documented limits:

| Class | Default budget |
| --- | --- |
| READ | 20/min, burst 10, 500/hour |
| SEARCH | 5/min, burst 2, 150/hour |
| WRITE | 1 per 30 s, 30/hour, 1 in flight at a time |
| GIT_CREDENTIAL | 1 per 10 s, 120/hour |

Anything gh-paced does not recognise is charged as a WRITE. A wait longer than
`GH_PACED_MAX_WAIT` (default 900 s) is refused with exit status 75, and so is a
single call costing more than its class's burst (a paginated write or search,
or a list `--limit` above 1,000 items), since one gh call makes those requests back
to back. Watch loops (`gh pr checks --watch`, `gh run watch`) are refused with
exit status 64 unless `GH_PACED_ALLOW_WATCH=1` is set; poll with repeated plain
calls instead.

git's credential helper (`gh auth git-credential get`, a GIT_CREDENTIAL call)
never waits long, because git may hold its caller's locks meanwhile: during a
cooldown it is refused at once, otherwise it waits at most
`GH_PACED_GIT_MAX_WAIT` (default 30 s), its waits for the state lock included.
Either refusal exits 75 (70 for a state lock that was not free in time) and
prints `quit=1`, so git stops instead of prompting; so does any other failure
before gh starts, a gh that cannot be executed (exit 127) included.

Run `gh-paced userguide` for the full reference: the classes, the citations
behind each budget, the configuration options, and every message.
