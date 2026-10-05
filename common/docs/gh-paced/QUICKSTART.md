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
- pauses every call for 15 minutes after any rate-limit or abuse response;
- refuses write bodies larger than 8 KiB or containing long base64 runs.

Every process on the host shares the budgets through one locked state file. The
command's own output, exit status, stdin and terminal pass through unchanged.

**Dependencies.** Linux, the real `gh` binary, and a Rust toolchain to build.

## Install

```bash
cargo build --release --manifest-path rs/Cargo.toml -p gh-paced
install -m 0755 rs/target/release/gh-paced ~/bin/gh-paced
```

## Use

Call it directly, naming the account whose budgets apply:

```bash
gh-paced --account octocat -- pr view 12 --json state
```

Or make your `gh` wrapper script hand every call to it:

```bash
exec ~/bin/gh-paced --account octocat --real-gh /usr/bin/gh -- "$@"
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
`GH_PACED_MAX_WAIT` (default 900 s) is refused with exit status 75.

Run `gh-paced userguide` for the full reference: the classes, the citations
behind each budget, the configuration options, and every message.
