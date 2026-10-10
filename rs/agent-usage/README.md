# agent-usage

Plan usage, reset times and burn rate for the Claude Code and Codex CLIs, without a model call.

```bash
cargo build --release --manifest-path rs/Cargo.toml -p agent-usage
agent-usage               # report (polls when the newest reading is over 2 minutes old)
agent-usage --json        # for agents and scripts
agent-usage daemon --detach   # 15-minute history for burn rates
```

`agent-usage quickstart` and `agent-usage userguide` are the operator documentation (also in
[`common/docs/agent-usage/`](../../common/docs/agent-usage/QUICKSTART.md)).

## How a reading is taken

| Provider | Source | Cost per reading |
| --- | --- | --- |
| Claude Code | `GET /api/oauth/usage` with the stored claude.ai login, the endpoint behind `/status` → Usage | one `curl` process (≈6 ms locally) plus one HTTPS round trip |
| Codex | `codex app-server`, JSON-RPC `account/rateLimits/read` | ≈0.6-0.8 s wall, ≈1.2-1.4 s CPU, ≈410 MB peak RSS (the app-server), freed on exit |
| Local tokens | Claude transcripts (incremental index), Codex `threads.tokens_used` via `sqlite3` | included in the `status` numbers below |

Neither source uses tokens. Hosts whose harnesses run on an API key, a cloud provider or a
gateway have no plan windows; agent-usage reports them as `unavailable` without contacting
anything and still reports local tokens.

## Benchmarks

Measured 2026-10-10 on a Linux development host (many cores, NVMe, warm page cache) with
Claude Code 2.1.292 and codex-cli 0.159.3, both on gateway logins, so the Claude HTTPS request
itself could not be timed there (see "Not measured" below). Method: each scenario run N times
from a Python driver (`os.wait4` for CPU), medians reported; peak RSS of agent-usage itself from
GNU `time`.

| Scenario | N | Wall (median) | CPU (median) |
| --- | --- | --- | --- |
| `status`, sample fresh, transcript index warm | 30 | 10.3 ms | 9.0 ms |
| `status --json`, same | 30 | 9.7 ms | 8.6 ms |
| `status --cached --no-tokens` (history only) | 30 | 1.1 ms | 0.8 ms |
| `status`, cold transcript index (52 transcripts, 641 MB touched in the last 24 h; binary search skips older bytes) | 5 | 247 ms | 245 ms |
| `poll`, both providers, no subscription login (file checks, `sqlite3` total) | 20 | 8.1 ms | 7.7 ms |
| `poll --provider codex` with the app-server forced | 5 | 610 ms (582-841) | 1,182 ms |
| one `curl` process (file URL), the local part of a Claude reading | 20 | 5.7 ms | 5.5 ms |

agent-usage's own peak RSS is about 2.5 MB. Disk: about 255 bytes per history line (two lines
per poll: 96 polls a day at the 15-minute default is about 49 KB a day); the transcript index
was 168 KB for 52 active transcripts.

The warm `status` cost is dominated by walking the transcript directories (1,368 files) and
rewriting the index; `--no-tokens` drops it to about 1 ms.

### The alternative: scraping `/status` from a Claude Code TUI

The task asked what it would cost to start a session per poll, or keep a dummy session open,
and type `/status`. Measured by starting `claude` in tmux, waiting for the prompt, opening
`/status`, then idling; process-tree RSS is the sum over all processes (shared pages counted
more than once), CPU from `/proc/<pid>/stat` over the tree.

| | Full configuration (14 MCP servers, hooks, plugins) | `--bare --strict-mcp-config` |
| --- | --- | --- |
| Start to prompt | 2.1 s | 2.4 s |
| `/status` render after the prompt | 0.06 s | 0.06 s |
| Processes in the session tree | 22-25 | 3 |
| Resident memory, tree sum | 7.0-7.4 GB | 0.67-0.74 GB |
| Largest single process | 487 MB | 472 MB |
| CPU to start | ≈18 CPU-s | ≈1.5 CPU-s |
| CPU while idle | 52.7 CPU-s per 5 min (17.6 % of a core) | 1.6 CPU-s per 2 min (1.4 % of a core) |

Per day at a 15-minute interval:

| Approach | CPU per day | Memory held |
| --- | --- | --- |
| agent-usage daemon, Claude only | 96 curl requests: a few CPU-seconds (estimated, TLS not timed) | none between polls |
| agent-usage daemon, Codex app-server | 96 × 1.2 CPU-s ≈ 115 CPU-s | ≈410 MB for under a second per poll |
| TUI started per poll, full configuration | 96 × 18 CPU-s ≈ 1,730 CPU-s | 7 GB during each poll |
| Dummy TUI kept open, full configuration | ≈15,200 CPU-s (4.2 CPU-hours) | 7.4 GB permanently |
| Dummy TUI kept open, bare | ≈1,200 CPU-s | 0.7 GB permanently |

A TUI scrape also needs a screen parser that breaks with every UI change. On a gateway login it
would find nothing to scrape: the Usage tab there shows only session cost, verified on the same
host. Because both CLIs expose the data through a structured, free interface, agent-usage does
not scrape.

### Not measured

- The Claude HTTPS round trip to the usage endpoint: the measurement host has no claude.ai
  login, and probing the endpoint without one was declined. It is one TLS request; the local
  cost is the 6 ms `curl` start above plus the TLS handshake.
- A Codex ChatGPT login: the app-server path was timed up to the server's "authentication
  required" reply, which comes after start-up and account resolution. A real rate-limit read
  adds the backend request.

## Tests

`cargo test -p agent-usage`: 34 unit tests (burn arithmetic including resets, proration and
short histories; both reply parsers on fixtures; transcript parsing, incremental scan,
duplicate lines and binary search; paths; argument parsing) and 5 end-to-end tests that drive
the binary against a fake `curl`, a fake `codex app-server` and a fake `sqlite3`, including a
check that the bearer token is never on `curl`'s command line or in the cache directory.
