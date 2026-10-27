# Harness transcripts

Fixtures for #574 harness-adapters: the tail of one conversation transcript per harness, read by
the transcript readers in `src/harness/`. Each keeps the entry structure of real files written by
the harness on 2026-10-27 (field names, nesting, timestamp formats, how a tool call and its result
are paired, Muse's transaction frames whose children carry records as JSON text). Every text,
command, path, id and model label was replaced; the conversations were written for the tests.
Times start at 2026-10-27T10:00:00Z.

| File | Harness | What the tail ends on |
|---|---|---|
| `claude.jsonl` | Claude Code session JSONL (`~/.claude/projects/<project>/<session>.jsonl`) | A gate run with a 600 s timeout and its result, then a sub-agent (`Agent`) call with no timeout still in flight, a background shell, and later bookkeeping (an attachment, a queue operation). |
| `codex.jsonl` | Codex rollout (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`) | A code-mode `exec` with a 30 s yield, a `wait_agent` with a 600 s timeout, then an unbounded `fetch_page` call still in flight and a later `token_count`. |
| `muse.jsonl` | Muse journal (`~/.local/share/muse/sessions/YYYY/MM/DD/<session>/session.jsonl`) | A bash call with a 30 s yield and its result, then a bash call with no limit still in flight and a later resource sample. |
