# slack-outbound-format: comparison harness and results

[#530 slack-outbound-format](https://github.com/rrnewton/agent-utils/issues/530)

Two outbound paths stay selectable until there is a clear winner:

- `markdown` (default): raw Markdown to the vendor CLI as `--markdown`;
  Slack converts it server-side.
- `converted`: `agentctl-slack-outbound` converts Markdown itself (comrak)
  and posts `--slack-mrkdwn`, split at block boundaries under 4,000 bytes.

Vendor check (2026-10-26, read-only): `meta slack.message send --help`
offers `--markdown` (12,000 bytes), `--text` (12,000), `--slack-mrkdwn`
(4,000) and `--blocks` (up to 50 Block Kit blocks); `--dry-run` previews
both mrkdwn and blocks posts, and `meta slack.message validate` accepts a
mrkdwn body. Detailed help says `--text`/`--slack-mrkdwn` keep every line
break while `--markdown` joins single line breaks, and that a Block Kit
table block exists. The converted path still renders tables as an aligned
monospace code block, because a native Slack table does not come back in
the client's text (the reading side drops it), so only the code block
round-trips through `slack.message list --content-format=text`.

## Corpus

Scrubbed samples in `corpus/` (no names or IDs):

- `01-report.md`: headings, a nested-bullet table, long lists, a task list,
  a block quote, a fenced code block, links and escapes — the shape of the
  report the vendor conversion damaged.
- `02-simple-chat.md`: two short paragraphs.

## Dry-run (CI)

```bash
cargo run --manifest-path rs/Cargo.toml -p chat-subscription-slack \
  --example slack_format_compare -- \
  ai_docs/transient/2026-10-26-slack-outbound-format/corpus/01-report.md \
  ai_docs/transient/2026-10-26-slack-outbound-format/corpus/02-simple-chat.md
```

Result on 2026-10-26 (this run, dry-run only, nothing posted):

| sample | blank lines | headings | links | table in code block | tasks | parts | max part bytes |
|---|---|---|---|---|---|---|---|
| 01-report.md | 12/12 | 2/2 | 1/1 | true | 2/2 | 1 | 731 |
| 02-simple-chat.md | 1/1 | 0/0 | 0/0 | n/a (no table) | 0/0 | 1 | 56 |

(The harness's fence count for 01-report.md reads 4 against a source 2
because the table's code block adds a fence pair.)

## Live mode

Live posting is never the default and was **not** run for this change.
The harness refuses live mode without `--channel=C…` for a test channel
plus `SLACK_FORMAT_COMPARE_LIVE=1`, and the lane must tell the coordinator
before any live post. When approved, post each sample by both paths, read
back with `meta slack.message list --content-format=text` (and mrkdwn),
and score blank lines kept, table intact, headings and links as in the
dry-run table.

## Capture fidelity (part 0)

The broken line in the original report came from the bridge capturing a
fenced reply from pane rows: rows filled to the pane width (162 columns)
were stored as hard newlines. The current build still captured that way
(reply bodies joined pane rows with newlines, while the reply-command path
bypasses capture), so `reply_body` now joins a row filled to the block's
widest row when the next row continues its sentence and starts no list,
heading, quote, table or code structure — and never inside a code fence.
Covered by `soft_wrapped_pane_rows_are_joined_but_fenced_rows_are_not` and
`a_captured_reply_joins_its_soft_wrapped_rows` in `agentctl`.
