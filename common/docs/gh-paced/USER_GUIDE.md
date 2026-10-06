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
  are untouched. The one exception is `gh api -i/--include`, whose stdout is
  copied through so its response headers can be read (see
  [pushback](#github-pushback-and-the-cooldown)).
- **stderr** is copied through while it is scanned for GitHub pushback (see
  below). When stderr is a terminal, gh sees a pseudo-terminal, so colours and
  prompts behave as before. A copied stream is scanned before it is passed on,
  and every byte gh-paced reads from gh reaches the consumer however slowly the
  consumer reads; gh feels the same back-pressure it would without gh-paced
  once 8 MiB is queued. After gh exits, gh-paced keeps reading until end of
  file, 2 s of silence, 5 s after the exit, or 64 MiB more, whichever comes
  first, because a background process gh started may hold the stream open.
  Anything that process writes after that is lost (see
  [Limitations](#limitations)).
- **A signal after gh has exited** (INT, TERM, HUP or QUIT, sent to gh-paced
  or typed at its terminal, while gh-paced is still delivering gh's output)
  drops the output not yet delivered, and gh-paced dies by that signal
  promptly, even when the program reading its stdout or stderr has stopped
  reading. Its warning is printed only if stderr accepts it within 2 s. A
  signal while gh is still running goes to gh instead (see the exit status
  below); if gh then exits while the reader is stalled, gh-paced goes on
  waiting for the reader, and a second signal ends it. A signal that arrives
  within a few milliseconds of gh's exit, before gh-paced has noticed the exit,
  has no effect; send it again. KILL ends gh-paced at once.
- **stdin** and the controlling terminal are inherited. The one exception: a
  write whose body arrives on stdin (`--body-file -`, `--input -`, `-F body=@-`)
  is read first so the content guard can check it, then replayed to gh byte for
  byte. A READ that sends stdin (`api -X GET ... --input -`) gets the inherited
  stdin itself, unread.
- **Body files** named by a write (`--body-file`, `-F field=@file`, `--input
  FILE`, gist files, and so on) are copied to a private snapshot before
  anything inspects them, and gh is given the copy. What the guard checked is
  therefore exactly what gh sends, even if the original changes during a long
  pacing sleep. See [State](#state-the-audit-log-and-status).
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
| LOCAL | `help`, `--version`, `--help` after one of gh's own commands, `completion`, `config`, `alias`, the help topics (`environment`, `formatting`, `reference`, ...), `auth token`, `auth switch`, `auth setup-git`, `auth git-credential store/erase` | free, not audited |
| READ | `gh api` with GET, HEAD or OPTIONS; `gh api graphql` whose query has no `mutation`; `pr view/list/status/checks/diff/checkout`; `issue view/list/status`; `run view/list/download/watch`; `workflow view/list`; `repo view/list/clone/set-default/gitignore/license`; `release view/list/download/verify`; the `list` and `view` forms of `label`, `gist`, `secret`, `variable`, `cache`, `ssh-key`, `gpg-key`, `org`, `project`, `ruleset`, `codespace`, `extension`; `auth status`; `browse`; `credits` | 1 |
| SEARCH | `gh search ...`, `gh extension search`, `gh api search/...` | 1 |
| SEARCH | `gh status` (it runs several search and GraphQL queries) | 3 |
| WRITE | `gh api` with POST, PATCH, PUT or DELETE; `gh api` with any `-f`, `-F`, `--raw-field`, `--field` or `--input` and no explicit `-X GET` (gh sends those as POST); a GraphQL mutation, or a GraphQL request that cannot be inspected (a query on stdin, `--input -`, a nested `query[...]` field); `pr create/comment/edit/merge/close/reopen/review/ready`; `issue create/comment/edit/close/reopen/delete/transfer/lock`; `label`, `release`, `gist`, `secret`, `variable` changes; `workflow run`; `run rerun/cancel`; `repo create/edit/delete/fork`; **every command gh-paced does not recognise** (aliases, extensions, new gh subcommands), even with `--help`, because an alias or extension may turn that argument into anything | 1 |
| GIT_CREDENTIAL | `gh auth git-credential get`, which git calls before each network operation | 1 |

For a GraphQL call, every document gh might send is inspected: each `-f/-F
query=...` value (an `@file` is read) and, with `--input FILE`, the file's
`query` string and its raw text. The call is READ only when none of them
contains a mutation.

Some calls cost more than one token:

- `gh api --paginate` (or `--slurp`) fetches every page back to back. It costs
  10 tokens (`paginate_cost`) and prints a warning. This applies to writes too:
  a paginated write can repeat once per page, so it is charged 10 WRITE tokens.
  At the default WRITE budget (2 per minute) the next write then waits about
  five minutes.
- `-L/--limit N` on a list or search costs one token per 100 items requested,
  because gh fetches up to 100 items per request.
- Watch loops, `gh pr checks --watch` and `gh run watch`, keep polling for as
  long as they run. They cost 20 tokens up front (`watch_cost`), and the tokens
  buy a fixed run time: `floor((cost - 2) / requests per poll) x interval`
  seconds. 2 tokens pay for startup (finding the run or pull request); a
  `gh run watch` poll is charged 4 requests (gh fetches the run, its workflow
  and its jobs, and more for large or failing runs) and a `gh pr checks --watch`
  poll 2. Both are estimates: a large or failing run can make more requests
  than its poll is charged (see [Limitations](#limitations)). At 30 s intervals that is 120 s for `run watch` and 270 s for
  `pr checks --watch`. A watch still running at that deadline is stopped (TERM,
  then KILL 5 s later) and gh-paced exits 75. A polling interval shorter than
  30 s is refused (exit 64) unless `GH_PACED_ALLOW_FAST_WATCH=1` is set. Pass
  `--interval 30` instead. Every `--interval` value must be a plain positive
  whole number of seconds (`30`, not `0`, `-1`, `030` or `0x1e`); anything else
  is refused (exit 64) even with the override, because gh treats zero or a
  negative interval as no sleep at all.
- A call whose cost is larger than its class's hourly cap can never be
  admitted, so it is refused at once (exit 75) rather than after a wait.

When a flag that sets a cost or a limit is repeated (`--interval`, `-L`), the
most conservative value counts, whichever occurrence gh honours. Short flags
are read the way gh reads them, including groups: `pr list -dL1000` is
`-d -L 1000`.

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
  once every 60 s (`rate_limit_min_refresh_secs`), never during a cooldown, and
  only one process on the host runs it at a time.
- **Cost.** One READ token, charged under the lock before the request is sent,
  so a refresh can never push the host past its READ budget. When a refresh is
  due and the READ bucket or hourly cap has no room for that token, READ,
  SEARCH and WRITE calls wait for room (or are refused past
  `GH_PACED_MAX_WAIT`) rather than run without current account numbers. The
  refresh gives up after 20 s.
- **After a sleep.** Every admission pass re-reads the state, so a call that
  slept (for a bucket, a cooldown, or a write slot) checks the account-wide
  numbers again, refreshing first if a refresh has come due, before it is
  admitted.
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

For `gh api -i/--include`, gh prints the HTTP status line and response headers
on **stdout** instead. For that form only, gh-paced also reads stdout as it
passes through, looking only inside header blocks: a 403 or 429 status,
`Retry-After`, and `X-RateLimit-Remaining: 0`. gh prints a header block as an
`HTTP/<version> <status>` line, then header lines ending in CRLF, then a CRLF
blank line. stdout itself is passed on unchanged.

- **One response (no `--paginate`).** gh prints exactly one header block, and
  it is the first thing on stdout. gh-paced reads that first block and nothing
  after it, so a response body cannot start a cooldown, whatever it contains.
  The block counts even if it is cut off before its blank line (the stream
  ended, or a line of another shape followed), so a cut-off block keeps its
  `Retry-After`.
- **`--paginate`.** gh prints one header block per page, each after the
  previous page's body, so gh-paced reads every block it finds in the shape
  above. A block counts when its CRLF blank line arrives, or when the output
  ends after at least one of its CRLF header lines. Body lines that merely look
  like a status line and headers, such as lines printed through `--jq`, end in
  a bare LF and are not treated as a header. A page body that itself contains
  that exact shape with CRLF line endings is read as one more header block and
  can start a cooldown that was not needed. gh-paced cannot tell such a body
  from a real page boundary, so this errs on the safe side.

Any limit signal (one of the phrases, HTTP 429, a `Retry-After` value, or
`X-RateLimit-Remaining: 0`) starts a **cooldown** of the larger of `Retry-After`
and 900 s (`cooldown_secs`). A plain `HTTP 403` starts a cooldown of
`plain_403_cooldown_secs`, also 900 s. Neither can be configured below 900 s:
GitHub does not always say that a 403 is a rate limit, so every 403 is treated
as one. During a cooldown, every paced call for that account on that host waits
or is refused. A new cooldown only ever extends an existing one. gh's own output
and exit status still pass through. The cooldown is recorded as soon as the
signal is read from gh's output, before that output is passed on, so other
calls on the host pause at once even if the program reading this call's output
is slow or has stopped reading. The banner below is printed after gh's output
has been passed on.

The banner looks like this:

```text
GH-PACED ********************************************************************
GH-PACED PUSHBACK [octocat] GitHub refused or throttled `api GET repos/o/r/issues/7/comments`: secondary rate limit, HTTP 403
GH-PACED PUSHBACK [octocat] every paced gh call for this account on this host is paused for 900 s, until 3:51 AM ET
GH-PACED PUSHBACK [octocat] STOP making GitHub calls and tell your coordinator. Do not retry in a loop. Never switch accounts to continue.
GH-PACED ********************************************************************
```

Apart from `--include` header blocks, gh-paced does not scan stdout. Error text
from GitHub arrives on stderr, and stdout often carries the caller's data
(`--json` output, `gh api` responses) that may legitimately contain these words.

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
- for an alias, extension or unknown command, every argument, flag-shaped or
  not (`--payload=<text>`, `--repo <text>`, `-- <text>`), and every occurrence
  of a repeated flag rather than only the last, because its expansion may pass
  any of them to a body flag;
- for any other write, `--body`, `--body-file` and `--input`.

Every body file is first copied to a private snapshot (see
[State](#state-the-audit-log-and-status)), and the guard and gh both read the
copy. A file that cannot be copied (missing, unreadable, or larger than 64 MiB,
a cap that applies even with `GH_PACED_ALLOW_LARGE_BODY=1`) is refused with
exit 65.

It refuses the call (exit 65), before anything reaches GitHub, when either
condition holds:

- the sources total more than **8,192 bytes** (`max_body_bytes`; a config file
  may lower it to as little as 256, never raise it);
- any source contains **base64-looking content longer than 1,000 characters**
  (`max_base64_run`; a config file may lower it to as little as 100, never raise
  it).

Base64-looking content is either of:

- one unbroken run of base64-alphabet characters (letters, digits, `+`, `/`,
  `=`, `_`, `-`), whatever it contains. A long hex dump, a lower-case-only
  encoding, and a long ruler of dashes all count, so nothing long slips through
  as a "word";
- a block of consecutive lines, each at least 20 characters and made only of
  base64-alphabet characters, counted together, whatever they contain. The
  usual 64- and 76-column wrapping therefore does not hide an encoding;
- a block of consecutive base64-alphabet lines of any width that all have the
  same width, except that the last may be shorter, counted together. This is
  the shape of an encoding wrapped at a fixed column, so wrapping at 16 or 8
  columns does not hide one either. Lines of differing widths, such as a list
  of test names one per line, do not form this kind of block.

Lists can match these shapes too. Bare commit SHAs one per line are refused at
26 or more full 40-character SHAs, or 143 or more 7-character short SHAs; a
column of 4-digit numbers one per line is refused at 251 lines. Put a word on
each line (`<sha> fix the parser`), or point at a commit range, instead.

The refusal names every reason that applies. Files are read only up to the
limit plus one byte, so a very large file is reported as "at least" that size:

```text
GH-PACED REFUSED [octocat] content guard: write body is 9000 bytes across 1 source(s), over the 8192-byte limit
GH-PACED REFUSED [octocat] not running `issue comment` (exit 65)
GH-PACED REFUSED [octocat] GitHub text is for short human notes. Keep evidence on the host and post a pointer (path + sha256, a tracked file, or a commit).
```

`GH_PACED_ALLOW_LARGE_BODY=1` skips the content guard for one call (the size and
base64 checks, and the refusals in the next subsection; the 64 MiB snapshot cap
still applies). Use it only for genuinely human-written long text, such as a
long design comment, and never for encoded or machine-generated payloads.

### Bodies gh composes itself

Some write forms make gh build the body after gh-paced has run, so the guard
cannot see it. These are refused with exit 65 unless
`GH_PACED_ALLOW_LARGE_BODY=1` is set, which skips the whole content guard:

- `pr create` / `issue create` with `--template`/`-T`, `--fill`,
  `--fill-first`, `--fill-verbose`/`-f`, or `--editor`/`-e`;
- `pr comment` / `issue comment` with `--editor`/`-e`;
- on a terminal, the forms that prompt for the body: `create` or `comment`
  without `--body`/`--body-file` (or `--web`); `pr review` without
  `--approve`, `--request-changes`, `--comment` or a body; `pr edit` /
  `issue edit` with no flags; `pr merge` without `--merge`, `--squash` or
  `--rebase`; `release create` without `--notes`, `--notes-file` or
  `--generate-notes`;
- `release create --notes-from-tag` (gh reads the tag's annotation or commit
  message after gh-paced has run and sends it as the release notes);
- `gist edit` without `--add` or `--remove` (it opens an editor or replaces a
  file gh reads later).

Boolean flags count by their value, as gh reads them: `--editor=false` is not
the editor form, and `--web=false` is not the web form.

The fix is always to write the text first and pass it with `--body` or
`--body-file`.

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
A write that finds the slot taken checks again every 5 s and prints its warning
on every check, so a waiting write is never silent.

The slot lasts as long as the gh process doing the write, not as long as the
wrapper. An admitted write creates a lease file, `<account>.lease-<nonce>` in
the state directory, takes an exclusive `flock` on it, and passes that open file
to gh. The kernel keeps the lock while any process still has the file open: the
wrapper, gh, or anything gh started. So if the wrapper is killed with SIGKILL,
the gh it started keeps the slot until gh itself exits. Other processes test a
slot with a non-blocking shared `flock`; a lease that is no longer locked is
cleared, and so is a holder from an older state file whose process has died.

When the write ends but a process gh started still holds the lease (gh left a
background helper running), gh-paced closes its own copy and leaves the slot
taken; a later write clears it once that process has exited.

A gh-paced started inside a gh run by another gh-paced (for example by an alias
or extension that calls gh again) does not wait for its own parent's slot. It
must prove that it descends from the holder in two ways: the holder's nonce is in
`GH_PACED_INFLIGHT_CHAIN`, and it holds the holder's locked lease file, inherited
from the holder. The second is checked in `/proc/self/fdinfo`, where Linux lists
the `flock` only for the open file that took it, so opening the lease file
again (for example redirecting stdin from it) proves nothing, and neither does
setting the variable by hand. Nesting deeper than 8 levels is refused (exit 75).

A process counts as running only while its PID and start time both match and
it is not a zombie.

Caveats of the lease design, all of which err toward fewer calls except the
second:

- a long-lived background process that gh starts and that keeps the inherited
  file open keeps the slot taken until it exits; later writes wait and are
  refused after `GH_PACED_MAX_WAIT`;
- deleting a lease file by hand frees the slot while its gh is still running;
- a nested process that closed its inherited files waits for its parent like
  any other write.

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
refill is also clamped.

The safety settings have floors that no config file can weaken. A value outside
its range is a configuration error (exit 78), not a silent clamp:

| Key | Allowed range | Default |
| --- | --- | --- |
| `cooldown_secs` | at least 900 | 900 |
| `plain_403_cooldown_secs` | at least 900 | 900 |
| `block_below_fraction` | 0.2 to 0.9, and no larger than `halve_below_fraction` | 0.2 |
| `halve_below_fraction` | 0.5 to 1.0 | 0.5 |
| `max_body_bytes` | 256 to 8,192 | 8,192 |
| `max_base64_run` | 100 to 1,000 | 1,000 |
| `rate_limit_refresh_secs` | 60 to 300 | 300 |
| `rate_limit_refresh_calls` | 1 to 50 | 50 |
| `rate_limit_min_refresh_secs` | 10 to 300 | 60 |
| `write.max_in_flight` | 1 to 4 | 1 |
| `paginate_cost`, `watch_cost` | at least 1 | 10, 20 |

Only `GH_PACED_ALLOW_LARGE_BODY=1`, set for one call, loosens the content
guard.

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
  step. It is never held across a sleep or a network call. The clock is read
  after the lock is taken, so a process that waited for the lock never charges
  at a stale time.
- `<account>.audit.jsonl` is the append-only audit log, rotated to `.1` past
  10 MiB.
- `<account>.cooldown` is a copy of the latest pushback cooldown, kept apart
  from the state file. Every load takes the later of the two, so a damaged
  state file cannot shorten a long `Retry-After` pause.
- `<account>.lease-<nonce>` is one file per running write; its lock is the
  write's in-flight slot (see [Waiting and refusing](#waiting-and-refusing)).
  It is removed when the write ends, unless a process gh started still holds
  it.
- `snap-<pid>-<start ticks>-<nonce>/` holds one invocation's private copies of
  the body files it sends (directories 0700, files 0600, at most 64 MiB per
  file), plus a `.lock` file that the invocation locks and gh inherits. It is
  removed when the invocation ends, unless a process gh started still holds
  that lock. Every paced call also removes directories whose lock is free,
  whose creating process has exited, and that are at least 60 s old, so one left
  behind by a killed wrapper does not linger; any other `snap-*` directory is
  removed once it is a day old.

The state file is replaced atomically (written to a temporary file, then
renamed). A state file that cannot be parsed is kept for inspection as
`<account>.json.corrupt-<unix-seconds>` (a hard link; `-1`, `-2`, ... is
appended if that name is taken) and replaced, by one rename, with a recovery
state, so the state path never goes missing and a crash during recovery cannot
leave a fresh start. Because the lost file may have held a full hour of calls,
gh-paced does not start from empty buckets: in the recovery state **every class
is blocked for the next hour** (a fixed end time, so raising a limit afterwards
does not shorten it), a cooldown of `cooldown_secs` is in force, and every write
whose lease is still locked is restored as in flight. An unreadable
`<account>.cooldown` file is moved aside the same way and triggers the same
recovery, because the pause it held is unknown. Paced calls for that account on
that host are therefore refused or delayed for the next hour, with a message
that says which file was unusable and where it was moved. It clears itself
after an hour; do not delete state files to get around it.

Each audit line records the UTC and US Eastern time, host, PID, event
(`admit`, `throttle`, `refuse`, `pushback`, `deadline`, `exit`, `refresh`),
class, cost, command, a redacted argument summary, exit status, seconds waited,
and a short detail. In the argument summary, inline bodies are replaced by
`<N bytes>`, header values are redacted, and a `gh api` endpoint loses its host,
user name and password, query string and fragment (a query string can carry an
`access_token`). For an alias, extension or unknown command, only flag names are
kept and every other argument becomes `<arg>`; an unrecognised flag of a known
command keeps its name and loses its value (`--token=X` becomes `--token`).
A `gh api` call carrying a flag gh-paced does not recognise is recorded as
`api <unparsed>`, because its endpoint cannot be told apart from that flag's
value. The log never contains request bodies, tokens or environment
variables. LOCAL calls are not audited.

`gh-paced status [--account NAME | --all] [--json]` reads these files and prints
each class's tokens and burst, rate, use in the last hour, and time to the next
one-token slot. It also prints in-flight writes, any cooldown, the cached GitHub
numbers and their age, and the last 5 audit records. It takes no lock, creates
no files, never contacts GitHub and never changes state; it can run while every
other process is stuck.

## Exit status

| Status | Meaning |
| --- | --- |
| gh's own | the call ran; gh-paced returns gh's status, or dies by gh's signal |
| 75 | refused: the wait would exceed `GH_PACED_MAX_WAIT`, the cost can never fit under the hourly cap, a watch ran past the deadline its cost paid for, or gh-paced is nested too deep |
| 65 | refused by the write content guard: body too large, base64-looking content, a body gh would compose itself, or a body file that cannot be copied for inspection |
| 64 | usage error, or a refused command shape (a watch interval under 30 s, or one that is not a positive whole number) |
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
- **The in-flight slot is held by a lock on a file.** Deleting a lease file by
  hand frees the slot early; see [Waiting and refusing](#waiting-and-refusing).
- **Watch charges are estimates.** gh-paced sees a watch's command line and
  output, not the HTTP requests gh makes on each poll, so it cannot count them;
  the requests per poll are worked out from what gh fetches. A `gh run watch`
  on a run with more than 100 jobs, or with several jobs that fail during the
  watch, can make more requests than it paid for before its deadline; the
  deadline still bounds how long it runs and the 30 s floor how often it polls.
- **Pushback detection depends on gh's error text.** gh-paced recognises the
  phrases GitHub and gh use today. A change in that wording could hide a
  pushback, but GitHub's account-wide counters (above) still apply.
- **A signal can arrive too early or too late to end gh-paced at once.** A
  signal sent while gh runs goes to gh; if gh exits but the program reading
  the output has stopped reading, gh-paced keeps waiting until a second signal.
  A signal in the few milliseconds between gh's exit and gh-paced noticing it
  is lost. See [What passes through unchanged](#what-passes-through-unchanged).
- **Output written long after gh exits is lost.** gh-paced stops reading a
  stream 2 s after it goes quiet, 5 s after gh exits, or after 64 MiB more. A
  background process gh started that writes later loses that output (it gets
  a write error, or SIGPIPE), and any pushback text in it is not seen. gh-paced
  cannot know whether such a process will write again: waiting for the stream
  to close would hold gh's exit status until, for example, a browser that
  `--web` opened is closed.
- **Header and encoding detection work by shape.** With `--paginate`, a page
  body containing a CRLF header block can start an unneeded cooldown. The
  content guard can refuse a long list of equal-width tokens one per line (see
  [pushback](#github-pushback-and-the-cooldown) and
  [the write content guard](#the-write-content-guard)).
