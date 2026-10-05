# gh-paced: design note (2026-10-05)

Working design record for `rs/gh-paced`. The settled user documentation is in
`common/docs/gh-paced/` (`QUICKSTART.md`, `USER_GUIDE.md`, `RELATED_WORK.md`),
and the binary prints it with `gh-paced quickstart` and `gh-paced userguide`.
This note records why the design looks the way it does, the evidence behind the
default budgets, the incident replay, and where the implementation departs from
the original request.

## Background and terms

- **gh** is the GitHub CLI. Each `gh` invocation makes one or more HTTPS
  requests to GitHub's REST or GraphQL API. gh has no client-side pacing: a loop
  over `gh` runs as fast as GitHub answers.
- **Primary rate limit**: GitHub's per-account hourly allowance, 5,000 REST
  requests and 5,000 GraphQL points per hour for a user token. `GET /rate_limit`
  reports how much of it is left.
- **Secondary rate limits**: further limits GitHub applies to bursts, such as
  concurrent requests, points per minute, and "content-generating" requests
  (anything that creates content: comments, issues, reviews). Some of these are
  undisclosed.
- **Token bucket**: a counter that refills at a fixed rate up to a maximum (the
  burst). A call spends tokens; when the bucket is empty, the next call waits.
- **Cooldown**: a period in which gh-paced runs no calls at all for an account
  on a host, started when GitHub refuses or throttles a call.
- **Snapshot**: the last `GET /rate_limit` result, cached in the state file.

## Why it exists

On 2026-10-04 a publishing script, using the shared bot account, posted 22
comments to one pull request: a 4,048-byte summary and 21 request bodies that
each carried a chunk of a base64-encoded archive. It read each comment back
immediately after posting it. Its own logs show 44 calls (22 POSTs and 22 GETs)
starting within 38.5 s, from 11:04:12.356 to 11:04:50.841 ET; the 22 POSTs
themselves took 37.49 s. The final read-back failed with GitHub's account-wide
"API rate limit exceeded" error, because other polling on the same account had
already used the hour's allowance. GitHub suspended the bot account shortly
afterwards. A suspended account's issues, pull requests and comments are hidden.

The owner's personal, long-term account is what remains, and losing it is not
recoverable. The owner asked for a wrapper that paces gh on the client side,
prints loud warnings when it does, and keeps two to four machines sharing one
account under GitHub's limits.

Corrections to earlier informal accounts of the incident: it was 44 calls, not
43; it lasted 38.5 s, not about 60 s; and the part request files were 60,601 to
60,602 bytes (60,000-character base64 lines), with a last part of 30,982 bytes
(a 30,380-character line), not "60.5 KB each".

## Design summary

**Placement.** gh-paced is a release-optimised Rust binary in the `rs/`
workspace. An account shim (a script that selects the account's credentials and
then runs gh) replaces its final `exec <real gh> "$@"` with
`exec gh-paced --account <name> --real-gh <real gh> -- "$@"`. gh-paced runs the
real gh as a child and returns its exit status (or dies by its signal). stdin,
stdout and stderr pass through; if stderr is a terminal the child gets a
pseudo-terminal for stderr, so gh still sees a TTY while gh-paced scans it.

**Classification.** Each command line is classified without touching the
network:

| Class | Examples | Cost |
| --- | --- | --- |
| LOCAL | `--help`, `version`, `completion`, `config get`, `auth git-credential store/erase` | not paced, not audited |
| READ | `pr view`, `issue list`, `run view`, `api` GET, GraphQL `query` | 1; `--paginate` 10; `--limit N` ceil(N/100) |
| SEARCH | `search ...`, `api search/...`, `extension search`, `status` | 1 (`status` 3) |
| WRITE | `pr create`, `issue comment`, `api -X POST`, GraphQL `mutation`, anything unknown | 1 |
| GIT_CREDENTIAL | `auth git-credential get` | 1 |

Unknown commands, aliases, extensions and unreadable GraphQL queries are WRITE.
`pr checks --watch` and `run watch` cost 20 and are refused below a 30 s
interval.

**State.** One JSON state file per account on each host
(`~/.local/state/gh-paced/<account>.json`), updated under an `flock` on a
separate `<account>.lock` and replaced atomically. The lock is never held across
a sleep or a network call. Writes in flight are recorded with the holder's PID,
process start time and a nonce, so a dead holder is reaped.

**Admission.** A call is admitted when its bucket holds enough tokens (or is
full, for a call that costs more than the burst) and the cost fits the
class's sliding one-hour cap. The full cost is charged, so an expensive call
leaves the bucket in debt. Otherwise gh-paced computes the wait (the longest of
cooldown, account-wide block, free write slot, refill, and hourly cap), prints
one `GH-PACED WARNING` line on stderr with the next slot in US Eastern time, and
sleeps. If the time already slept plus the next wait exceeds
`GH_PACED_MAX_WAIT` (default 900 s), it refuses with exit 75 and runs nothing.

**Account-wide feedback.** gh-paced refreshes the snapshot when none exists,
when it is at least 300 s old, or after 50 admitted calls, at most once per 60 s
and by one process at a time. The refresh costs one READ token. At 50% or less
remaining (on `core`/`graphql`, plus `search` for SEARCH) the READ, SEARCH and
WRITE budgets are halved; at 20% or less those calls block until the resource
resets.

**Pushback.** gh's stderr is scanned with a 4 KiB overlap for `secondary rate
limit`, `rate limit`, `HTTP 429`, `abuse`, `submitted too quickly`,
`Retry-After: N`, and plain `HTTP 403`. Any of them starts a cooldown of
max(Retry-After, 900 s), printed as a banner, and discards the snapshot.

**Content guard.** Before a WRITE runs, every body source is read: inline
flags, `--body-file`, `-F field=@file`, `--input FILE`, stdin, gist files,
release notes files. A JSON request body counts at its full size, and every
string inside it is also decoded and scanned. The call is refused (exit 65)
when the total exceeds 8,192 bytes or any source has a base64-looking run
longer than 1,000 characters. The run detector also joins consecutive
base64-only lines, so line-wrapped encodings are caught.
`GH_PACED_ALLOW_LARGE_BODY=1` skips it for one call.

**Audit and status.** Each paced call appends a JSON line (time in UTC and ET,
host, PID, event, class, cost, command, redacted argv, waited seconds, detail)
to `<account>.audit.jsonl`. No bodies, tokens or environment are written.
`gh-paced status` prints the buckets, in-flight writes, cooldown, snapshot and
last 5 records from local files only.

## Budgets and the GitHub limits behind them

Defaults per host, per account:

| Class | Per minute | Burst | Per hour | Other |
| --- | --- | --- | --- | --- |
| READ | 20 | 10 | 500 | |
| SEARCH | 5 | 2 | 150 | |
| WRITE | 2 (1 per 30 s) | 1 | 30 | 1 in flight per host |
| GIT_CREDENTIAL | 6 (1 per 10 s) | 1 | 120 | |

GitHub's documented limits, quoted verbatim:

- [Rate limits for the REST API](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api):
  "All of these requests count towards your personal rate limit of 5,000
  requests per hour." "No more than 100 concurrent requests are allowed."
  "No more than 900 points per minute are allowed for REST API endpoints, and
  no more than 2,000 points per minute are allowed for the GraphQL API
  endpoint." "In general, no more than 80 content-generating requests per minute and no
  more than 500 content-generating requests per hour are allowed. Some
  endpoints have lower content creation limits." "These secondary rate limits
  are subject to change without notice." "If the retry-after response header is
  present, you should not retry your request until after that many seconds has
  elapsed. ... Otherwise, wait for at least one minute before retrying."
  "Continuing to make requests while you are rate limited may result in the
  banning of your integration."
- [Search](https://docs.github.com/en/rest/search/search): "30 requests per
  minute for all search endpoints except for the Search code endpoint."
- [GraphQL rate limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api):
  "For users: 5,000 points per hour per user."
- [Rate limit endpoint](https://docs.github.com/en/rest/rate-limit/rate-limit):
  "Accessing this endpoint does not count against your REST API rate limit."

### Fleet arithmetic: four hosts on one account

| Budget | Four hosts at the default | GitHub's limit | Share |
| --- | --- | --- | --- |
| READ per hour | 4 x 500 = 2,000 calls | 5,000 requests | 40% |
| READ per minute | 4 x 20 = 80 | 900 points | 9% |
| SEARCH per minute | 4 x 5 = 20 | 30 | 67% |
| WRITE per minute | 4 x 2 = 8 | 80 content-generating | 10% |
| WRITE per hour | 4 x 30 = 120 | 500 content-generating | 24% |
| Writes in flight | 4 x 1 = 4 | 100 concurrent (all kinds) | 4% |

Caveats. A gh call can make more than one HTTP request; at two per call, four
hosts at their full READ budget make 4,000 requests an hour, 80% of the primary
limit. Calls that do not go through gh-paced (the web UI, Actions running as the
user, a script's own HTTP client) are invisible to the local budgets; the
snapshot is the only feedback that covers them. Between two refreshes, four
hosts can spend at most about 4 x 20 x 5 = 400 READ tokens (8% of 5,000), or
200 when halved, which is why the blocking floor is 20% rather than lower.

SEARCH is the tightest share (67%). It is still under the limit, and the
snapshot halves it when the search pool drops to 50%.

## Incident replay

All of these run in `cargo test -p gh-paced` (`tests/replay.rs`) with a fake gh
and a fake clock, so no network is used. The fake gh records each invocation at
the fake time; sleeps advance the fake clock. Numbers below are the test's own
`eprintln!` output from one run.

### (a) The content guard refuses the archive parts

`incident_parts_are_refused_and_the_summary_is_allowed` replays the original
sequence (summary, read-back, part, read-back, ...) with request files shaped
like the originals: a JSON object whose `body` holds one long base64 line. The
summary is allowed; all 21 parts are refused with exit 65 before gh runs:

```text
GH-PACED REFUSED [replay] content guard: write body is 60074 bytes across 1 source(s), over the 8192-byte limit; --input contains a base64-looking run of at least 8127 characters (limit 1000); never store encoded data in GitHub text
GH-PACED REFUSED [replay] not running `api POST repos/o/r/issues/1/comments` (exit 65)
```

The "at least" is because files are read only up to the limit plus one byte.
The run finished at fake time +39.9 s. gh ran 23 times (1 POST, the summary,
and 22 GETs) plus 1 snapshot refresh. The READ window held 23 tokens (22 GETs
and the refresh) and the WRITE window 1.

The real incident request files were also run through the release binary once,
read-only, with a fake gh and a scratch state directory:

| Configuration | Summary (4,048 B) | Parts 01-21 | Example refusal |
| --- | --- | --- | --- |
| defaults | exit 0 | all 21 exit 65 | part 01: 60,601 bytes over 8,192, base64 run of at least 7,603 |
| `max_body_bytes` 65,536 | | 01 and 21 exit 65 | base64 runs of 60,001 and 30,381 |
| `max_base64_run` 100,000 | | 01 and 21 exit 65 | 60,601 and 30,982 bytes over 8,192 |

Each check alone refuses the parts it was run on. In the first row the fake gh's
log shows one snapshot refresh and one POST, the summary; no part reached it. In
the last row the fake gh's log file was never created: a refused call reaches
neither gh nor the snapshot refresh.

### (b) Prose bodies are paced

`prose_bodies_are_spaced_30_seconds_apart_and_gets_use_the_read_bucket` replays
the same 44-call sequence with 1,000-byte prose bodies, which pass the guard:

- 22 POSTs ran from +0.3 s to +630.3 s: a span of 630.0 s, minimum gap
  30.001 s. The requirement was at least 21 x 30 s = 630 s (10.5 min). The
  original sequence took 37.49 s for the same 22 POSTs.
- The first wait printed
  `GH-PACED WARNING [replay] write budget 1 per 30 s (burst 1) per host reached; sleeping 29 s`.
- Total sleep: 604.2 s. The 22 GETs ran from the READ bucket between the
  writes and were never charged as writes: the WRITE window held 22 tokens and
  the READ window 22 plus the 3 snapshot refreshes.

`hourly_write_cap_holds` posts 31 writes in sequence. The first 30 run (one per
30 s). The 31st is refused under the default 900 s bound with
`write budget 30/hour per host reached (30 used in the last hour)`; with a
4,000 s bound it is admitted at +3,600.0 s, and no hour ever holds more than 30.

### (c) A depleted account blocks every API call

`low_account_budget_blocks_every_api_call_loudly` sets the snapshot to 900 of
5,000 core requests left (18%) with the reset 1,800 s away. A POST, a GET,
`pr view`, `search issues` and a GraphQL query are each refused with exit 75, and
gh is never started:

```text
GH-PACED WARNING [replay] account-wide core budget at 18% (900/5000 left until 11:34 AM ET); local read rates halved to 10/min and 250/hour
GH-PACED REFUSED [replay] account-wide core budget at 18% (900/5000 left), at or below the 20% floor; blocking until it resets
GH-PACED REFUSED [replay] the next slot is in 1800 s (at 11:34 AM ET), beyond GH_PACED_MAX_WAIT=900 s
GH-PACED REFUSED [replay] not running `api GET repos/o/r/issues/comments/1009` (exit 75)
```

A caller that allows a 3,600 s wait sleeps to the reset and then runs.
`half_account_budget_halves_local_rates` checks the 50% step: the READ budget
becomes 10/min and 250/hour, and the halved burst forces a wait.

Fleet read volume: four hosts at the default READ budget can make 4 x 500 =
2,000 paced gh calls an hour, 40% of the 5,000-request primary limit (80% if
every call made two requests). The incident's exhaustion came from traffic
outside any pacing; under gh-paced, the snapshot would have reported the low
remaining count and blocked these calls before the read-back that failed.

### (d) Where this lives

This section is the record. The tests above are part of `make validate`: the
step `gh-paced` / `test` in `validation/rust.dag.yaml` (reported as
`rust.gh-paced.test`) runs them, and two path rules in `scripts/validate.py`
select it. `rs/gh-paced/` selects the Rust gate and the Rust workspace tests;
`common/docs/gh-paced/` selects those plus the docs checks, because the binary
compiles its guides in with `include_str!`. gh-paced is deliberately not a
component in `validation/components.json`: every component there must name at
least one Python test file, and gh-paced has no Python side.

## The cooldown and the wait bound

After a pushback the cooldown is 900 s, and `GH_PACED_MAX_WAIT` also defaults to
900 s. A call made at the start of a fresh cooldown therefore needs a 900 s wait,
which is not more than the bound, so it **sleeps the full 15 minutes** and then
runs. It is refused only once the time already slept plus the remaining wait
exceeds the bound. This is by design: the request was "issue sleeps", and a
caller that would rather fail fast sets a smaller `GH_PACED_MAX_WAIT`. It does
mean an agent can appear to hang for 15 minutes after a pushback; the warning
line it prints first says so and gives the end time.

## Live smoke test

Run on 2026-10-05 against GitHub with the owner's personal account, through the
release binary, with a scratch state directory and a two-line `--real-gh` script
that calls the existing account shim. The shim itself was not modified.

| Time (ET) | Call | Result |
| --- | --- | --- |
| 03:49:23 | `api rate_limit` (no proxy) | exit 1: TCP connect failed ("network is unreachable"); nothing reached GitHub |
| 03:49:40 | `api rate_limit` | exit 0; core 5000/5000, graphql 5000/5000, search 30/30 |
| 03:49:56 | snapshot refresh (`api rate_limit`, made by gh-paced) | parsed: core 5000/5000 left (100%) |
| 03:50:08 | `api repos/<owner>/agent-utils --jq .full_name` | exit 0, printed the repository name |

Three requests reached GitHub, 16 s and 12 s apart. The second call was run with
`GH_PACED_READ_PER_MINUTE=5 GH_PACED_READ_BURST=1`, so after the refresh took
the only READ token the GET had to wait. gh-paced printed
`GH-PACED WARNING [<account>] read budget 1 per 12 s (burst 1) per host reached; sleeping 12 s (next slot 3:50 AM ET)`
and the call took 12.8 s. `gh-paced status` afterwards showed the snapshot
(19 s old, core and graphql resetting at 4:49 AM ET) and the audit records
`exit`, `refresh`, `throttle`, `admit`, `exit`. The account had used none of its
primary allowance in that hour.

## Departures from the request, with reasons

1. **stdout is not scanned for pushback.** GitHub's errors arrive on gh's
   stderr; stdout carries the caller's data (`--json`, `gh api` bodies) that may
   legitimately contain "rate limit" or "abuse".
2. **GraphQL is classified by its query.** `api graphql` with a readable
   `query` is READ; a `mutation`, or a query that cannot be read (from a file
   gh-paced cannot open, say), is WRITE. Treating every GraphQL call as WRITE
   would make read-only tooling unusable at one call per 30 s.
3. **List and view subcommands are split from writes** per command family
   (`label list` READ, `label create` WRITE; `gist view` READ, `gist create`
   WRITE; `extension list` READ, `extension install` WRITE).
4. **`auth git-credential store` and `erase` are LOCAL.** Only `get` returns a
   credential for a network operation; git calls `store`/`erase` after it, and
   they do not contact GitHub.
5. **Update notifiers are disabled for the child.** gh-paced sets
   `GH_NO_UPDATE_NOTIFIER=1` and `GH_NO_EXTENSION_UPDATE_NOTIFIER=1` unless the
   caller set them, because the release check is an unpaced request.
6. **Watch loops are charged and fast ones refused.** `pr checks --watch` and
   `run watch` cost 20 tokens and are refused (exit 64) below a 30 s interval,
   unless `GH_PACED_ALLOW_FAST_WATCH=1`.
7. **`--limit N` costs ceil(N/100) tokens** on list and search commands, the
   number of 100-item pages it may fetch.
8. **`gh status` is SEARCH with cost 3.** It runs several search and GraphQL
   queries.
9. **Environment variables only tighten** a budget; a looser value is ignored
   with a warning. `GH_PACED_MAX_WAIT` is the exception: it sets how long the
   caller will wait, not how fast GitHub is called. Config-file values are
   clamped to GitHub's documented ceilings.
10. **stderr goes through a pseudo-terminal when it is a terminal**, so gh keeps
    its interactive behaviour while gh-paced scans the stream.
11. **A plain HTTP 403 has its own cooldown setting**
    (`plain_403_cooldown_secs`, default 900 s, as requested). Permission errors
    are also 403s; a workflow that hits them often can shorten it without
    weakening the rate-limit cooldown.
12. **Exit statuses** beyond 75: 65 for the content guard, 64 for usage errors,
    70 for state errors, 78 for configuration errors (including a `--real-gh`
    that resolves to gh-paced), 127 when the real gh is missing.
13. **A corrupt state file is quarantined** to `<account>.json.corrupt-<secs>`
    and replaced with empty buckets, which errs toward fewer calls.
14. **LOCAL calls are not audited**, to keep the log about GitHub traffic.
15. **GIT_CREDENTIAL is not blocked by the snapshot.** git operations do not use
    the API pools the snapshot reports; they keep their own bucket and the
    pushback cooldown still applies.
16. **Nested gh-paced** (an extension calling gh) skips its own parent's write
    slot and refuses past 8 levels (exit 75).

## Test changes worth a reviewer's attention

- `concurrent_processes_share_one_write_budget` (in `tests/cli.rs`) checks the
  write spacing on the admission times gh-paced records under the shared lock,
  not on the fake gh's own start times, which include bash start-up jitter. The
  spread of the fake gh's start times is still asserted (at least 1.8 s for three
  writes at one per second).
- The `ratelimit` unit test's post-reset expectations were corrected during
  development. The rule they check: a resource whose reset has passed counts as
  full; READ and WRITE use `core` and `graphql`; SEARCH also uses `search`.
- Bugs found and fixed during development: a floating-point refill that could
  spin on a sub-millisecond wait; the child inheriting gh-paced's blocked signal
  mask; overriding a signal the caller had set to ignored (as `nohup` does); a
  guard that reported only its first reason; misaligned `status` columns.

## Open items

- Budgets are per host; hosts do not share state. More than four hosts on one
  account need a tighter config file.
- Classification works from command lines, not HTTP requests.
- Pushback detection depends on gh's error wording.
