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
- pauses every call for 15 minutes after any rate-limit, abuse or HTTP 403
  response;
- refuses write bodies larger than 8 KiB or containing long base64 runs,
  whether they come from an argument, a file, stdin, one of gh's aliases, or
  the editor gh opens (gh-paced points gh's editor at a guard that checks the
  saved text);
- refuses writes whose body gh would compose from something it reads later
  (a template, `--fill`, the commit-message default of an interactive
  `pr create`): pass `--body` or `--body-file` instead.

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
`GH_PACED_MAX_WAIT` (default 900 s) is refused with exit status 75, and so is a
single call costing more than its class's burst (a paginated write or search,
or a list `--limit` above 1,000 items), since one gh call makes those requests back
to back. Watch loops (`gh pr checks --watch`, `gh run watch`) are refused with
exit status 64 unless `GH_PACED_ALLOW_WATCH=1` is set; poll with repeated plain
calls instead.

Run `gh-paced userguide` for the full reference: the classes, the citations
behind each budget, the configuration options, and every message.
