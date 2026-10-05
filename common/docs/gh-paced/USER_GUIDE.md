# gh-paced user guide

`gh-paced` is a client-side rate limiter for the GitHub CLI. It sits between the
caller (a person, a script, an agent, or an account wrapper script) and the real
`gh` binary. Each call is classified, charged against a budget, and delayed with
a loud stderr warning when that budget is spent. Otherwise the call passes
through unchanged.

## Why it exists

`gh` itself does no pacing. Its only related feature is the response cache,
`gh api --cache <duration>`. A loop that calls `gh` runs as fast as GitHub
answers. GitHub limits this in two ways (see [Budgets](#budgets-and-the-github-limits-behind-them)):

- **Primary limits** are counters you can read: 5,000 REST requests per hour,
  5,000 GraphQL points per hour, and 30 search requests per minute, per user.
- **Secondary limits** cannot be read in advance. They cover concurrency,
  requests per minute, CPU time, and how much content is created. GitHub states:
  "Continuing to make requests while you are rate limited may result in the
  banning of your integration."

When several hosts use the same account, their calls add up against the same
limits. gh-paced keeps each host's share small enough that four hosts together
stay well below the documented numbers. It also reacts to GitHub's own signals
when the total gets close anyway.

## Installing and wiring it in

Build it and copy the binary somewhere on your PATH:

```bash
cargo build --release --manifest-path rs/Cargo.toml -p gh-paced
install -m 0755 rs/target/release/gh-paced ~/bin/gh-paced
```

gh-paced needs to know two things: which account's budget applies, and where the
real `gh` is.

- **Account.** `--account NAME`, or `$GH_PACED_ACCOUNT`. This is required.
  Budgets are kept separately for each account.
- **Real gh.** `--real-gh PATH`, or `$GH_PACED_REAL_GH`. Otherwise gh-paced uses
  `/usr/bin/gh`, then `/usr/local/bin/gh`. It never searches PATH. It refuses a
  path that resolves to gh-paced itself (exit 78), so a wrapper cannot loop.

Usually an existing account wrapper script hands every call to gh-paced as its
last step:

```bash
#!/bin/bash
# ~/bin/gh: select the account, then pace every call.
GH_PACED="$HOME/bin/gh-paced"
[ -x "$GH_PACED" ] || { echo "gh: $GH_PACED is missing; refusing to call GitHub unpaced" >&2; exit 127; }
exec "$GH_PACED" --account octocat --real-gh /usr/bin/gh -- "$@"
```

Everything after `--` is passed to gh exactly as given. git's credential helper
setting (`!gh auth git-credential`) goes through the same wrapper. That means
`git fetch` and `git push` are paced too, as GIT_CREDENTIAL calls.

## What passes through unchanged

- **stdout** goes straight to the real gh's stdout, so pipes and `--json` output
  are untouched.
- **stderr** is copied through while it is scanned for GitHub pushback (see
  below). When stderr is a terminal, gh sees a pseudo-terminal, so colours and
  prompts behave as before.
- **stdin** and the controlling terminal are inherited. The one exception: a
  write whose body arrives on stdin (`--body-file -`, `--input -`, `-F body=@-`)
  is read first so the content guard can check it, then replayed to gh.
- **The exit status** is gh's own. If gh is killed by a signal, gh-paced dies by
  the same signal, without a core dump. INT, TERM, HUP and QUIT sent to gh-paced
  are forwarded to gh. A signal that was ignored when gh-paced started (for
  example HUP under `nohup`) stays ignored in gh.
- gh-paced sets `GH_NO_UPDATE_NOTIFIER=1` and `GH_NO_EXTENSION_UPDATE_NOTIFIER=1`
  unless the caller set them, because the update checks are unpaced API calls.

gh-paced's own exit statuses are listed under [Exit status](#exit-status).

## Request classes

Each call gets exactly one class. Each class except LOCAL has its own token
bucket and hourly window. `gh-paced classify -- <args>` prints the class, the
cost, and the reason for any command line, without running it.

| Class | Covers | Cost |
| --- | --- | --- |
| LOCAL | `help`, `--help`, `--version`, `completion`, `config`, `alias`, the help topics (`environment`, `formatting`, `reference`, ...), `auth token`, `auth switch`, `auth setup-git`, `auth git-credential store/erase` | free, not audited |
| READ | `gh api` with GET, HEAD or OPTIONS; `gh api graphql` whose query has no `mutation`; `pr view/list/status/checks/diff/checkout`; `issue view/list/status`; `run view/list/download/watch`; `workflow view/list`; `repo view/list/clone/set-default/gitignore/license`; `release view/list/download/verify`; the `list` and `view` forms of `label`, `gist`, `secret`, `variable`, `cache`, `ssh-key`, `gpg-key`, `org`, `project`, `ruleset`, `codespace`, `extension`; `auth status`; `browse`; `credits` | 1 |
| SEARCH | `gh search ...`, `gh extension search`, `gh api search/...` | 1 |
| SEARCH | `gh status` (it runs several search and GraphQL queries) | 3 |
| WRITE | `gh api` with POST, PATCH, PUT or DELETE; `gh api` with any `-f`, `-F`, `--raw-field`, `--field` or `--input` and no explicit `-X GET` (gh sends those as POST); a GraphQL mutation, or a GraphQL query that cannot be inspected (on stdin); `pr create/comment/edit/merge/close/reopen/review/ready`; `issue create/comment/edit/close/reopen/delete/transfer/lock`; `label`, `release`, `gist`, `secret`, `variable` changes; `workflow run`; `run rerun/cancel`; `repo create/edit/delete/fork`; **every command gh-paced does not recognise** (aliases, extensions, new gh subcommands) | 1 |
| GIT_CREDENTIAL | `gh auth git-credential get`, which git calls before each network operation | 1 |

Some calls cost more than one token:

- `gh api --paginate` (or `--slurp`) fetches every page back to back. It costs
  10 tokens (`paginate_cost`) and prints a warning. A write with `--paginate` is
  still one write.
- `-L/--limit N` on a list or search costs one token per 100 items requested,
  because gh fetches up to 100 items per request.
- Watch loops, `gh pr checks --watch` and `gh run watch`, keep polling for as
  long as they run. They cost 20 tokens up front (`watch_cost`). A polling
  interval shorter than 30 s is refused (exit 64) unless
  `GH_PACED_ALLOW_FAST_WATCH=1` is set. Pass `--interval 30` instead.

## Budgets and the GitHub limits behind them

Each budget is a **token bucket** plus a **sliding one-hour cap**. The bucket
holds up to `burst` tokens and refills at `per_minute`. A call is admitted when
the bucket holds enough tokens (or is full, for a call that costs more than the
burst) *and* the cost fits under `per_hour` in the last 3,600 seconds. The full
cost is always charged, so an expensive call leaves the bucket in debt and later
calls wait until it refills.

Default budgets, per host, per account:

| Class | per_minute | burst | per_hour | Other |
| --- | --- | --- | --- | --- |
| READ | 20 | 10 | 500 | |
| SEARCH | 5 | 2 | 150 | |
| WRITE | 2 (one per 30 s) | 1 | 30 | at most 1 write in flight per host |
| GIT_CREDENTIAL | 6 (one per 10 s) | 1 | 120 | |

### What GitHub documents

From [Rate limits for the REST API](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api):

- "All of these requests count towards your personal rate limit of 5,000
  requests per hour."
- "No more than 100 concurrent requests are allowed. This limit is shared across
  the REST API and GraphQL API."
- "No more than 900 points per minute are allowed for REST API endpoints, and no
  more than 2,000 points per minute are allowed for the GraphQL API endpoint."
  Most GET, HEAD and OPTIONS requests cost 1 point; most POST, PATCH, PUT and
  DELETE requests cost 5.
- "No more than 90 seconds of CPU time per 60 seconds of real time is allowed."
- "In general, no more than 80 content-generating requests per minute and no
  more than 500 content-generating requests per hour are allowed. Some endpoints
  have lower content creation limits."
- "These secondary rate limits are subject to change without notice. You may
  also encounter a secondary rate limit for undisclosed reasons."
- On a secondary limit: "If the retry-after response header is present, you
  should not retry your request until after that many seconds has elapsed. ...
  Otherwise, wait for at least one minute before retrying."
- "Continuing to make requests while you are rate limited may result in the
  banning of your integration."

From [the search endpoints](https://docs.github.com/en/rest/search/search):
"30 requests per minute for all search endpoints except for the Search code
endpoint. The Search code endpoint requires you to authenticate and limits you to
10 requests per minute."

From [Rate limits and query limits for the GraphQL API](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api):
"For users: 5,000 points per hour per user."

From [the rate limit endpoint](https://docs.github.com/en/rest/rate-limit/rate-limit):
"Accessing this endpoint does not count against your REST API rate limit." The
REST guide adds that it "can count against your secondary rate limit".

### Four hosts on one account

| Class | Four hosts at the default budget | GitHub's documented limit | Share |
| --- | --- | --- | --- |
| READ per hour | 4 x 500 = 2,000 gh calls | 5,000 requests (REST) | 40% |
| READ per minute | 4 x 20 = 80 | 900 points per endpoint | 9% |
| SEARCH per minute | 4 x 5 = 20 | 30 | 67% |
| WRITE per minute | 4 x 2 = 8 | 80 content-generating | 10% |
| WRITE per hour | 4 x 30 = 120 | 500 content-generating | 24% |
| Concurrent writes | 4 x 1 = 4 | 100 concurrent requests (all kinds) | 4% |

Two caveats keep these numbers from being a guarantee:

1. **One gh call can make several HTTP requests.** For example, `pr view` with
   many `--json` fields, or `pr checks`, can make more than one. At two requests
   per call, four hosts at their full READ budget would make 4,000 requests an
   hour, 80% of the primary limit. The account-wide feedback below exists for
   this case.
2. **gh-paced sees only calls that go through it.** A browser session, the web
   UI, GitHub Actions running as the same user, or a script with its own HTTP
   client all draw on the same limits.

The WRITE budget is the strictest. GitHub's content-creation limit is the one
with undisclosed lower per-endpoint values, and content written by a suspended
account is hidden.

## Account-wide feedback: `GET /rate_limit`

Each host's budget is local. To see the whole account's position, gh-paced
regularly runs `gh api rate_limit` and caches the result in the state file. That
endpoint is free on the primary limit, but it counts against secondary limits,
so it is paced too:

- **When it runs.** A refresh is due when there is no snapshot, the snapshot is
  at least 300 s old (`rate_limit_refresh_secs`), or 50 calls have been admitted
  since the last one (`rate_limit_refresh_calls`). It never runs more often than
  once every 60 s (`rate_limit_min_refresh_secs`), and only one process on the
  host runs it at a time.
- **Cost.** One READ token. It gives up after 20 s.
- **Halving.** If the tightest relevant resource has 50% or less remaining
  (`halve_below_fraction`), READ, SEARCH and WRITE run at half their per-minute
  rate, burst and hourly cap. READ and WRITE look at `core` and `graphql`;
  SEARCH also looks at `search`.
- **Blocking.** At 20% or less remaining (`block_below_fraction`), every READ,
  SEARCH and WRITE call waits until that resource resets. If the reset is
  further away than `GH_PACED_MAX_WAIT`, the call is refused (exit 75).
  GIT_CREDENTIAL calls are not blocked: they do not use the API pools.
- **Staleness.** A snapshot older than one hour is ignored. A resource whose
  reset time has passed counts as full. Any pushback (next section) discards the
  snapshot so the next call fetches a fresh one.

Between two refreshes on one host, four hosts can spend at most about
4 x 20 x 5 = 400 READ tokens (8% of 5,000), or 200 when halved. The 20% blocking
floor therefore leaves room for calls that land before the next refresh.

## GitHub pushback and the cooldown

gh prints server refusals on stderr. gh-paced scans that stream as it passes
through, keeping a 4 KiB overlap between reads so a phrase split across two
reads is still found. It looks for:

- `secondary rate limit`, `rate limit` (including `API rate limit exceeded`),
  `HTTP 429`, the word `abuse`, `submitted too quickly`;
- `HTTP 403` with none of the above;
- a `Retry-After: N` value (the largest one seen).

Any of the limit phrases starts a **cooldown** of the larger of `Retry-After` and
900 s (`cooldown_secs`). A plain `HTTP 403` starts a cooldown of
`plain_403_cooldown_secs` (default also 900 s; set it lower if permission errors
are common in your workflow). During a cooldown, every paced call for that
account on that host waits or is refused. A new cooldown only ever extends an
existing one. gh's own output and exit status still pass through.

The banner looks like this:

```text
GH-PACED ********************************************************************
GH-PACED PUSHBACK [octocat] GitHub refused or throttled `api GET repos/o/r/issues/7/comments`: secondary rate limit, HTTP 403
GH-PACED PUSHBACK [octocat] every paced gh call for this account on this host is paused for 900 s, until 3:51 AM ET
GH-PACED PUSHBACK [octocat] STOP making GitHub calls and tell your coordinator. Do not retry in a loop. Never switch accounts to continue.
GH-PACED ********************************************************************
```

gh-paced does not scan stdout. Error text from GitHub arrives on stderr, and
stdout often carries the caller's data (`--json` output, `gh api` responses)
that may legitimately contain these words.

## The write content guard

GitHub issues, comments and pull-request descriptions are for short notes that
people read. They are not a place to store logs, archives or encoded data.
Before running a WRITE, gh-paced reads every body source:

- `pr`/`issue` `create`, `comment`, `edit`, and `pr review`: `--title`,
  `--body`, `--body-file` (including `-` for stdin), and the short forms;
- `pr merge`: `--subject`, `--body`, `--body-file`;
- `pr`/`issue` `close` and `reopen`: `--comment`;
- `release create/edit`: `--title`, `--notes`, `--notes-file`;
- `gist create` (its files and `--desc`) and `gist edit` (`--add`, `--desc`);
- `label create/edit` and `repo create/edit`: the name and description;
- `secret set`/`variable set`: `--body`, `--env-file`, or stdin;
- `gh api -f/--raw-field`, `-F/--field` (including `@file` and `@-`), and
  `--input FILE` (including `-`). A JSON request file counts at its full size,
  and every string inside it is also scanned after JSON decoding, so a `body`
  field is caught however it is escaped;
- `gh workflow run -f/-F` and `--json` (stdin);
- for any other write, `--body`, `--body-file` and `--input`.

It refuses the call (exit 65), before anything reaches GitHub, when either
condition holds:

- the sources total more than **8,192 bytes** (`max_body_bytes`, allowed range
  256 to 65,536);
- any source contains a **base64-looking run longer than 1,000 characters**
  (`max_base64_run`).

The refusal names every reason that applies. Files are read only up to the
limit plus one byte, so a very large file is reported as "at least" that size:

```text
GH-PACED REFUSED [octocat] content guard: write body is 9000 bytes across 1 source(s), over the 8192-byte limit
GH-PACED REFUSED [octocat] not running `issue comment` (exit 65)
GH-PACED REFUSED [octocat] GitHub text is for short human notes. Keep evidence on the host and post a pointer (path + sha256, a tracked file, or a commit).
```

`GH_PACED_ALLOW_LARGE_BODY=1` skips the guard for one call. Use it only for
genuinely human-written long text, such as a long design comment, and never for
encoded or machine-generated payloads.

## Waiting and refusing

When a call cannot be admitted yet, gh-paced works out how long to wait. That is
the longest of: the cooldown, the account-wide block, a free write slot, the
bucket refill, and the hourly cap. It then prints one warning and sleeps:

```text
GH-PACED WARNING [octocat] write budget 1 per 30 s (burst 1) per host reached; sleeping 28 s (next slot 3:36 AM ET)
```

If the total time slept plus the next wait would exceed `GH_PACED_MAX_WAIT`
(default 900 s), the call is refused instead. Nothing is sent to GitHub:

```text
GH-PACED ********************************************************************
GH-PACED REFUSED [octocat] write budget 30/hour per host reached (30 used in the last hour)
GH-PACED REFUSED [octocat] the next slot is in 1712 s (at 4:05 AM ET), beyond GH_PACED_MAX_WAIT=900 s
GH-PACED REFUSED [octocat] not running `issue comment` (exit 75)
GH-PACED ********************************************************************
```

Writes are also limited to `write.max_in_flight` (default 1) at a time per host.
A write that finds the slot taken polls every 2 s and repeats its warning every
30 s. A holder whose process has died is cleared automatically. A gh-paced
started inside a gh run by another gh-paced (for example by an extension) does
not wait for its own parent's slot. Nesting deeper than 8 levels is refused
(exit 75).

Times in messages are US Eastern by default. Set `GH_PACED_DISPLAY_TZ=UTC` to
print UTC instead.

## Configuration

Settings come from three layers, applied in order:

1. **Built-in defaults**, shown in the tables above.
2. **An optional JSON config file** at `$GH_PACED_CONFIG`, else
   `$XDG_CONFIG_HOME/gh-paced/config.json`, else
   `~/.config/gh-paced/config.json`. A missing default file is fine. A file
   named by `$GH_PACED_CONFIG` must exist. Unknown keys are an error (exit 78).
3. **Environment variables**, which can only **tighten** a limit. A looser value
   is ignored with a warning. The exception is `GH_PACED_MAX_WAIT`, which
   accepts any value because it decides how long a caller is willing to wait,
   not how fast GitHub is called.

Config file keys, with defaults:

```json
{
  "read":           { "per_minute": 20, "burst": 10, "per_hour": 500 },
  "search":         { "per_minute": 5,  "burst": 2,  "per_hour": 150 },
  "write":          { "per_minute": 2,  "burst": 1,  "per_hour": 30, "max_in_flight": 1 },
  "git_credential": { "per_minute": 6,  "burst": 1,  "per_hour": 120 },
  "paginate_cost": 10,
  "watch_cost": 20,
  "min_watch_interval_secs": 30,
  "max_wait_secs": 900,
  "cooldown_secs": 900,
  "plain_403_cooldown_secs": 900,
  "rate_limit_refresh_secs": 300,
  "rate_limit_refresh_calls": 50,
  "rate_limit_min_refresh_secs": 60,
  "rate_limit_timeout_secs": 20,
  "halve_below_fraction": 0.5,
  "block_below_fraction": 0.2,
  "max_body_bytes": 8192,
  "max_base64_run": 1000,
  "display_tz": "US-Eastern"
}
```

The config file is checked against GitHub's documented ceilings. A READ, SEARCH
or WRITE rate above GitHub's own number (for example WRITE above 80/min or
500/hour) is clamped to it with a warning. A burst of more than ten minutes of
refill is also clamped. Other bounds: `write.max_in_flight` 1 to 4;
`cooldown_secs` at least 60 (GitHub's "at least one minute");
`rate_limit_refresh_secs` at least 60; `rate_limit_min_refresh_secs` at least 10;
`block_below_fraction` 0.05 to 0.9, and no larger than `halve_below_fraction`.

Environment variables:

| Variable | Meaning |
| --- | --- |
| `GH_PACED_ACCOUNT` | default for `--account` |
| `GH_PACED_REAL_GH` | default for `--real-gh` |
| `GH_PACED_MAX_WAIT` | longest total sleep before refusing, seconds (default 900) |
| `GH_PACED_{READ,SEARCH,WRITE,GIT}_{PER_MINUTE,BURST,PER_HOUR}` | tighten one budget |
| `GH_PACED_PAGINATE_COST` | raise the `--paginate` cost |
| `GH_PACED_DISPLAY_TZ` | `US-Eastern` (default) or `UTC` |
| `GH_PACED_ALLOW_LARGE_BODY` | `1` skips the content guard for this call |
| `GH_PACED_ALLOW_FAST_WATCH` | `1` allows watch intervals under 30 s |
| `GH_PACED_STATE_DIR` | state directory (default `$XDG_STATE_HOME/gh-paced` or `~/.local/state/gh-paced`) |
| `GH_PACED_CONFIG` | config file path |

## State, the audit log, and `status`

State lives in the state directory (mode 0700, files 0600), one set of files per
account:

- `<account>.json` holds the buckets, the hourly windows, the in-flight writes,
  any cooldown, and the last `GET /rate_limit` snapshot.
- `<account>.lock` is taken with `flock` around every short read-modify-write
  step. It is never held across a sleep or a network call.
- `<account>.audit.jsonl` is the append-only audit log, rotated to `.1` past
  10 MiB.

The state file is replaced atomically (written to a temporary file, then
renamed). A state file that cannot be parsed is moved aside to
`<account>.json.corrupt-<unix-seconds>` and replaced with **empty** buckets.
Every class then refills from zero, which errs toward fewer calls.

Each audit line records the UTC and US Eastern time, host, PID, event
(`admit`, `throttle`, `refuse`, `pushback`, `exit`, `refresh`), class, cost,
command, a redacted argument summary, exit status, seconds waited, and a short
detail. Inline bodies are replaced by `<N bytes>` and header values are
redacted. The log never contains request bodies, tokens or environment
variables. LOCAL calls are not audited.

`gh-paced status [--account NAME | --all] [--json]` reads these files and prints
each class's tokens and burst, rate, use in the last hour, and time to the next
one-token slot. It also prints in-flight writes, any cooldown, the cached GitHub
numbers and their age, and the last 5 audit records. It never contacts GitHub
and never changes state.

## Exit status

| Status | Meaning |
| --- | --- |
| gh's own | the call ran; gh-paced returns gh's status, or dies by gh's signal |
| 75 | refused: the wait would exceed `GH_PACED_MAX_WAIT`, or gh-paced is nested too deep |
| 65 | refused by the write content guard |
| 64 | usage error, or a refused command shape (a watch interval under 30 s) |
| 70 | internal error: the pacing state cannot be locked, read or written |
| 78 | configuration error, or `--real-gh` resolves to gh-paced |
| 127 | the real gh cannot be found or run |

A caller that sees 75 should not retry immediately. The message says when the
next slot opens.

## Limitations

- **Budgets are per host.** Hosts do not share state. The defaults divide
  GitHub's limits by four. If more hosts share an account, tighten the budgets
  with a config file.
- **Only calls through gh-paced are paced.** Calling the real gh directly, curl,
  or a library's own HTTP client bypasses it.
- **Classification works from the command line.** It does not see the HTTP
  requests gh actually makes. Unknown commands are charged as WRITE, and a
  GraphQL request whose query cannot be read is charged as WRITE.
- **Pushback detection depends on gh's error text.** gh-paced recognises the
  phrases GitHub and gh use today. A change in that wording could hide a
  pushback, but GitHub's account-wide counters (above) still apply.
