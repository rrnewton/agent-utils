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
| LOCAL | `--help` after a known gh command, `version`, `completion`, `config get`, `auth git-credential store/erase` | not paced, not audited |
| READ | `pr view`, `issue list`, `run view`, `api` GET, GraphQL `query` | 1; `--paginate` 10; `--limit N` ceil(N/100) |
| SEARCH | `search ...`, `api search/...`, `extension search`, `status` | 1 (`status` 3) |
| WRITE | `pr create`, `issue comment`, `api -X POST`, GraphQL `mutation`, anything unknown (even with `--help`) | 1; `--paginate` 10 |
| GIT_CREDENTIAL | `auth git-credential get` | 1 |

Unknown commands, aliases, extensions and unreadable GraphQL queries are WRITE.
`pr checks --watch` and `run watch` cost 20, are refused below a 30 s interval,
and are stopped (exit 75) at `floor(cost / requests per poll) x interval`
seconds, so a watch never polls more than it paid for. A cost above the class's
hourly cap is refused at once.

**State.** One JSON state file per account on each host
(`~/.local/state/gh-paced/<account>.json`), updated under an `flock` on a
separate `<account>.lock` and replaced atomically. The lock is never held across
a sleep or a network call, and the clock is read only after it is taken. A
write's in-flight slot is a lease: a file `<account>.lease-<nonce>` with an
exclusive `flock` whose open file gh inherits, so the slot lasts until gh and
everything holding that file exit, even if the wrapper is SIGKILLed. A state
file that cannot be parsed is moved aside and replaced by a recovery state with
every hourly window full, a 900 s cooldown, and the still-locked leases
restored, so a lost hour of history is treated as fully used.

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
and by one process at a time, never during a cooldown. The refresh costs one
READ token, charged under the lock before the request is sent; with no READ
room it is skipped. The numbers are re-checked on every admission pass, so after
any sleep. At 50% or less
remaining (on `core`/`graphql`, plus `search` for SEARCH) the READ, SEARCH and
WRITE budgets are halved; at 20% or less those calls block until the resource
resets.

**Pushback.** gh's stderr is scanned with a 4 KiB overlap for `secondary rate
limit`, `rate limit`, `HTTP 429`, `abuse`, `submitted too quickly`,
`Retry-After: N`, and plain `HTTP 403`. For `api -i/--include`, stdout header
blocks are also read (status 403/429, `Retry-After`, `X-RateLimit-Remaining:
0`); response bodies are never matched. Any of them starts a cooldown of
max(Retry-After, 900 s), printed as a banner, and discards the snapshot. A plain
403 also gets 900 s, and neither cooldown can be configured lower.

**Content guard.** Before a WRITE runs, every body source is read: inline
flags, `--body-file`, `-F field=@file`, `--input FILE`, stdin, gist files,
release notes files. A JSON request body counts at its full size, and every
string inside it is also decoded and scanned. The call is refused (exit 65)
when the total exceeds 8,192 bytes or any source has a base64-looking run
longer than 1,000 characters. Any unbroken base64-alphabet run counts, hex
and rulers included; the detector also joins consecutive base64-only lines
that contain an upper-case letter, `+` or `/`, so line-wrapped encodings are
caught while a list of lower-case SHAs is not. Body files are first copied to a
private directory, and both the guard and gh read the copy, so a file cannot
change between the check and the send. Forms where gh composes the body itself after
the guard (editor, template, `--fill`, interactive prompt, `gist edit` without
`--add`/`--remove`) are refused with exit 65. The limits can only be lowered by
configuration. `GH_PACED_ALLOW_LARGE_BODY=1` skips the guard for one call.

**Audit and status.** Each paced call appends a JSON line (time in UTC and ET,
host, PID, event, class, cost, command, redacted argv, waited seconds, detail)
to `<account>.audit.jsonl`. No bodies, tokens or environment are written; an
endpoint loses its host, userinfo, query string and fragment, and an unknown
command keeps only its flag names. `gh-paced status` prints the buckets,
in-flight writes, cooldown, snapshot and last 5 records from local files only,
without taking the lock or creating files.

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
The run finished at fake time +39.6 s. gh ran 23 times (1 POST, the summary,
and 22 GETs) plus 1 rate-limit refresh. The READ window held 23 tokens (22 GETs
and the refresh) and the WRITE window 1.

The real incident request files were also run through the release binary,
read-only, with a fake gh and a scratch state directory:

| Binary | Configuration | Summary (4,048 B) | Parts 01-21 | Example refusal |
| --- | --- | --- | --- | --- |
| round 1 (uncommitted on `c06243e7`) | defaults | exit 0 | all 21 exit 65 | part 01: 60,601 bytes over 8,192, base64 run of at least 7,603 |
| round 1 | `max_body_bytes` 65,536 | | part 01 exit 78 | `max_body_bytes must be in 256..=8192, got 65536` |
| round 1 | `max_base64_run` 100,000 | | part 01 exit 78 | `max_base64_run must be in 100..=1000, got 100000` |
| `c06243e7` | `max_body_bytes` 65,536 | | 01 and 21 exit 65 | base64 runs of 60,001 and 30,381 |
| `c06243e7` | `max_base64_run` 100,000 | | 01 and 21 exit 65 | 60,601 and 30,982 bytes over 8,192 |

In the defaults row the fake gh's log shows one rate-limit refresh and one POST,
the summary, sent from the private copy
(`<state dir>/snap-<nonce>/0/SUMMARY.request.json`); no part reached it, and no
copy was left behind. The audit log of that run holds no base64-alphabet run of
100 characters or more. The round-1 binary refuses the two loosened
configurations outright (exit 78, the configuration floors), so the last two
rows, which show that each check alone refuses the parts, were measured with
the earlier binary, which accepted them. In those runs the fake gh's log file
was never created: a refused call reaches neither gh nor the rate-limit
refresh. The `guard` unit tests cover each check separately in the current
code.

### (b) Prose bodies are paced

`prose_bodies_are_spaced_30_seconds_apart_and_gets_use_the_read_bucket` replays
the same 44-call sequence with 4,096-byte prose bodies, which pass the guard:

- 22 POSTs ran from +0.3 s to +630.9 s: a span of 630.6 s, minimum gap
  30.000 s (the test requires at least 30 s less 1 microsecond). The requirement was at least 21 x 30 s = 630 s (10.5 min). The
  original sequence took 37.49 s for the same 22 POSTs.
- The first wait printed
  `GH-PACED WARNING [replay] write budget 1 per 30 s (burst 1) per host reached; sleeping 29 s`.
- Total sleep: 604.8 s. The 22 GETs ran from the READ bucket between the
  writes and were never charged as writes: the WRITE window held 22 tokens and
  the READ window 22 plus the 3 rate-limit refreshes.

`hourly_write_cap_holds` posts 31 writes in sequence. The first 30 run (one per
30 s). The 31st is refused under the default 900 s bound with
`write budget 30/hour per host reached (30 used in the last hour)`; with a
4,000 s bound it is admitted at +3,600.3 s, and no hour ever holds more than 30.

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
| 03:49:56 | rate-limit refresh (`api rate_limit`, made by gh-paced) | parsed: core 5000/5000 left (100%) |
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

1. **stdout is scanned only for `api --include` header blocks.** GitHub's
   errors arrive on gh's stderr; stdout carries the caller's data (`--json`,
   `gh api` bodies) that may legitimately contain "rate limit" or "abuse". For
   `api -i/--include`, whose status line and headers are on stdout, header
   blocks are read (403/429, `Retry-After`, `X-RateLimit-Remaining: 0`) and
   bodies are not. (Round 1 found that a `Retry-After: 3600` there was missed.)
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
6. **Watch loops are charged, bounded, and fast ones refused.** `pr checks
   --watch` and `run watch` cost 20 tokens, are refused (exit 64) below a 30 s
   interval unless `GH_PACED_ALLOW_FAST_WATCH=1`, and are stopped at
   `floor(cost / requests per poll) x interval` seconds (TERM, KILL after 5 s,
   exit 75), so a watch never polls beyond what it paid for. (Round 1 found the
   earlier unbounded watch.)
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
    (`plain_403_cooldown_secs`, default 900 s, as requested), but it has the
    same 900 s floor as the rate-limit cooldown. (Round 1 found it could be set
    to zero.)
12. **Exit statuses** beyond 75: 65 for the content guard, 64 for usage errors,
    70 for state errors, 78 for configuration errors (including a `--real-gh`
    that resolves to gh-paced), 127 when the real gh is missing.
13. **A corrupt state file is quarantined** to `<account>.json.corrupt-<secs>`
    and replaced with a recovery state: every class's hourly window full, a
    `cooldown_secs` pause, and every still-locked write lease restored. The
    account is paused on that host for an hour. (Round 1 showed that the
    earlier empty buckets could reopen an exhausted hourly budget or erase a
    cooldown.)
14. **LOCAL calls are not audited**, to keep the log about GitHub traffic.
15. **GIT_CREDENTIAL is not blocked by the snapshot.** git operations do not use
    the API pools the snapshot reports; they keep their own bucket and the
    pushback cooldown still applies.
16. **Nested gh-paced** (an extension calling gh) skips its own parent's write
    slot only when it proves descent twice: the parent's nonce is in
    `GH_PACED_INFLIGHT_CHAIN` and the parent's lease file is among its own
    inherited open files. It refuses past 8 levels (exit 75). (Round 1 found the
    chain variable alone could be forged.)
17. **Configuration cannot weaken the safety floors.** Cooldowns are at least
    900 s, blocking starts at 20% or higher, halving at 50% or higher, the body
    limit is at most 8,192 bytes and the base64 limit at most 1,000 characters,
    and the snapshot is refreshed at least every 300 s and 50 calls. A value
    outside its range exits 78. Only `GH_PACED_ALLOW_LARGE_BODY=1`, per call,
    loosens the guard.
18. **Writes whose body gh composes later are refused** (exit 65): editor,
    template, `--fill`, interactive prompts on a terminal, and `gist edit`
    without `--add`/`--remove`. The guard cannot see those bodies.

## Review round 1 and the fixes

Codex (gpt-6.1-sol, reasoning effort ultra, read-only sandbox) reviewed
`fb96152e..c06243e7` and requested changes: 1 blocker, 15 major, 2 minor. Each
finding, its fix, and the test that fails without the fix:

| # | Finding | Fix | Test |
| --- | --- | --- | --- |
| 1 | GraphQL `-f query=` preferred over `--input` | every candidate document is inspected; `--input -` is WRITE and its stdin is buffered and replayed | `classify::graphql_*`, `cli::stdin_reaches_gh_untouched` |
| 2 | unknown command with `--help` was LOCAL | `--help` is LOCAL only after a known gh command | `classify` unit tests (`my-alias --help` is WRITE) |
| 3 | bodies gh composes later escaped the guard | `guard::uninspectable` refuses them (exit 65); implicit `gist create` stdin is a source | `guard` unit tests |
| 4 | files re-read by gh after the guard | private file copy (`snapshot.rs`); gh gets the copy | `snapshot` unit tests |
| 5 | first occurrence of a repeated flag used | costs take the most conservative value; the guard keeps the last occurrence, as gh does | `classify`, `guard` unit tests |
| 6 | base64 detection required a digit | any unbroken base64-alphabet run counts; multi-line blocks need upper case, `+` or `/` | `guard::base64_detection` |
| 7 | paginated write cost 1 | costs `paginate_cost` (10) | `classify::paginate_and_limit_costs` |
| 8 | unbounded watch | deadline from the purchased polls | `cli::watch_past_its_deadline_is_stopped` |
| 9 | cost above the hourly cap admitted | refused at once (infinite wait) | `budget` unit test |
| 10 | clock read before the lock | clock read after `flock` | `replay::time_is_read_after_the_lock_is_taken` |
| 11 | in-flight slot tied to the wrapper PID | lease file lock inherited by gh; nested exclusion needs the inherited fd | `cli::killed_wrapper_leaves_its_orphaned_gh_holding_the_write_slot`, `cli::nesting_needs_the_inherited_lease_not_just_the_chain` |
| 12 | corrupt state reset to empty buckets | full hourly windows, cooldown, restored leases | `state` unit test, `replay::corrupt_state_pauses_the_account_for_an_hour` |
| 13 | refresh charged after it ran; not rechecked after sleeps | claimed and charged inside the admission loop | `replay::refresh_requests_are_charged_before_they_are_sent`, `replay::feedback_is_rechecked_after_a_sleep` |
| 14 | `--include` headers on stdout ignored | stdout header blocks scanned for `api -i` | `cli::include_headers_on_stdout_set_the_cooldown`, `pushback` unit tests |
| 15 | configuration could weaken protections | floors in `config::floors`, out-of-range is exit 78 | `config` unit tests |
| 16 | audit kept query strings and alias arguments | endpoint normalised; unknown commands keep flag names only | `cli::query_string_secrets_stay_out_of_the_records`, `guard` unit tests |
| 17 | in-flight wait warned every 30 s | polls every 5 s and warns on every poll | `replay::in_flight_wait_warns_on_every_poll`, `cli::only_one_write_in_flight_per_account` |
| 18 | `status` took the lock and created files | reads the atomically replaced file without locking | `cli::status_takes_no_lock_and_creates_nothing` |

Seven of these tests were also checked by mutation: re-introducing the old
behaviour (nested exclusion on the chain alone, clock before the lock, no stdout
scan, no deadline, the orphan check by PID only, `status` taking the lock, and a
warning only on the first in-flight check) makes the named test fail.

## Test changes worth a reviewer's attention

Round 1 changed these existing tests. Every change makes the test stricter or
follows a behaviour change the review asked for:

- `budget`: a cost above the hourly cap now gives an infinite wait (it was
  admitted into an empty window).
- `pushback`: the plain-403 cooldown in the test config went from 120 s to
  1,800 s, because 120 s is now below the 900 s floor and is a config error.
- `classify`: a paginated write costs 10 (was 1); `my-alias --help` is WRITE
  (was LOCAL); flag names in reasons are the canonical long forms.
- `guard`: base64 cases now include prose (longest run 5), a 1,600-character hex
  dump (counts 1,600), a 2,000-dash ruler (counts 2,000), and 4,000 repeated
  `x` (refused); before, digit-free runs were exempt.
- `state`: quarantine now expects saturated windows, a cooldown and restored
  leases (it expected empty buckets); `reap` returns the reaped holders.
- `config`: the floors are asserted; values that used to be accepted below them
  are now errors.
- `cli::stdin_reaches_gh_untouched`: a READ now proves 100,000 bytes of stdin
  reach gh unread; `api graphql --input -` is asserted to be WRITE and replayed
  byte for byte (an earlier comment called it READ, which was wrong).
- `IN_FLIGHT_POLL_SECS` went from 2 s to 5 s, with a warning on every poll
  instead of every 30 s.
- `replay::corrupt_state_pauses_the_account_for_an_hour` asserts the hourly
  budget text in the refusal, because a refusal names the longest wait (the
  3,600 s window, not the 900 s pause); the pause is asserted through the saved
  state file instead.

- `concurrent_processes_share_one_write_budget` (in `tests/cli.rs`) checks the
  write spacing on the admission times gh-paced records under the shared lock,
  not on the fake gh's own start times, which include bash start-up jitter. The
  spread of the fake gh's start times is still asserted (at least 1.8 s for three
  writes at one per second). Round 1 noted that those admission times were then
  sampled before the lock; they are now sampled after it, and
  `replay::time_is_read_after_the_lock_is_taken` checks that directly.
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
