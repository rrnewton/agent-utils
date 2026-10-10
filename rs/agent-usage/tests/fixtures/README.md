# Fixtures

These replies are synthetic. Their field names and shapes follow the response schemas that the
Claude Code and Codex CLIs bundle for the same endpoints (the claude.ai usage endpoint's legacy
named windows and its `limits[]` rows; Codex app-server's `account/rateLimits/read` result), with
made-up values. They contain no account data and no credentials.

- `claude-usage-limits.json`: a reply carrying server `limits[]` rows, including a per-model
  weekly row.
- `claude-usage-legacy.json`: a reply with only the legacy named windows and extra usage.
- `codex-ratelimits.jsonl`: a fake app-server conversation (one JSON-RPC message per line).
- `claude-transcript.jsonl`: Claude Code transcript lines, one message split over two lines.
