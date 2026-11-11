# slack-outbound-format: live comparison (2026-10-26)

[#530 slack-outbound-format](https://github.com/rrnewton/agent-utils/issues/530).
The owner approved this run. Identifiers (channel, message timestamps, users,
apps, hosts) are scrubbed.

## Setup

- Bridge: one production agentctl Slack bridge, upgraded to main `cc234e08`
  (agentctl, `agentctl-slack-plugin`, `agentctl-slack-outbound`), with
  `"slack_format": "converted"` as its default from this run on.
- Posts: one root message (`agentctl chat publish`, through the bridge's
  converted default), then four replies in its thread with
  `agentctl chat publish --thread-id …`. The replies were each corpus sample
  (`corpus/01-report.md`, `corpus/02-simple-chat.md`) posted once with
  `--slack-format markdown` and once with `--slack-format converted`. Each
  reply's first line is a one-line label, then a blank line, then the sample
  verbatim.
- Readback (read-only):
  1. the vendor CLI's `slack.message list --thread-ts=…` with
     `--content-format=mrkdwn`, `--content-format=text`, and the default
     (Markdown), plus the `rich_content` column;
  2. a vibe-talk Slack instance's own API, `GET
     /api/v1/channels/{id}/timeline?view=thread`: its cached `content`
     (rebuilt by its Slack shim) and the `content_html` the page draws.
- The helper's diagnostics log confirmed each post's path:
  `slack_format=markdown chunks=1` or `slack_format=converted chunks=1`.

## Scores: `01-report.md`

"Slack text" means the stored text as the CLI returns it (mrkdwn and text
agree on structure). "vibe-talk" means the rendered `content_html`.

| feature (source count) | markdown: Slack text | markdown: vibe-talk | converted: Slack text | converted: vibe-talk |
|---|---|---|---|---|
| blank lines between blocks (12) | 4/12 | 4/12 (the shim adds 2 of its own around the code fence and table note) | 12/12 | 12/12 |
| headings (2) | 0/2: the heading text runs into the next line (`Weekly report Status is…`, `Numbers 1. First…`) | 0/2: same, inside a paragraph | 2/2 as bold lines | 2/2 as `<strong>` paragraphs (not `<h1>`/`<h2>`) |
| table (1, 2 rows) | missing from the text; a native table block exists (`rich_content` has its rows), and every readback (mrkdwn, text, Markdown) appends `, with interactive elements` | missing: `[a table in this message could not be read; open it in Slack]` | aligned monospace code block, intact | `<pre>` block, intact, right-aligned column kept |
| bullet list + nested (1+2) | intact | the paragraph after the table is glued into the last nested item | intact | intact |
| numbered list (3) | glued to the heading line, no blank line before it | rendered as one paragraph, not `<ol>` | intact | `<ol>`, 3 items |
| task list (2) | checkboxes dropped; the done item became ~~strikethrough~~ | same | `☐` / `☑` kept (Slack stores `☑` as the `:ballot_box_with_check:` shortcode) | `☐` / `☑️` |
| link (1) | 1/1 | 1/1 | 1/1 | 1/1 |
| block quote (1) | kept, no blank line after it | kept | kept | kept |
| fenced code (1) | the mrkdwn and text readbacks return `` ```fn main() {…}``` `` with no newline after the opening fence (the default Markdown readback shows a proper fence); language tag dropped | **broken**: the first code line is read as the info string, the fence never closes, and the rest of the message (final line, inline code) becomes code | fenced block on its own lines; language tag dropped | `<pre>` block, intact |
| inline code, emphasis, `&` | kept | inline code lost (swallowed by the code block) | kept | kept |
| posts | 1 | | 1 | |

## Scores: `02-simple-chat.md`

Both paths read back identically: two paragraphs, 1/1 blank line, the same
text in every CLI readback and in vibe-talk's cached `content` (the API
returned no `content_html` for these two messages, so their drawn form was
not captured).

## Findings

1. **Converted wins on every structural feature measured**, in both readers.
   The markdown path loses 8 of 12 block breaks, both headings, the table
   (from the text), the task boxes and the numbered list's separation. In
   vibe-talk it also corrupts everything after the code block, because the
   mrkdwn readback the shim uses puts the code's first line on the opening
   fence's line.
2. **Headings are bold lines, not headings**, on the converted path. That is
   by design (Slack mrkdwn has no headings), but a reader cannot tell a
   heading from a bold sentence. Follow-up below.
3. **The markdown path's native table never reaches a reader that uses
   text.** `rich_content` carries the table's rows, yet this vibe-talk
   instance's shim still showed its "could not be read" note for this
   message. The CLI's `rich_content` for that message (present in the mrkdwn and text
   readbacks) also holds neither heading and stops after the paragraph that
   follows the table: the numbered list, tasks, quote, code and final line
   are absent from it, though present in the text. Unexplained; not investigated here.
4. **Short chat is unaffected** by the choice.
5. **Not verified:** how the official Slack client draws each post. It
   draws the markdown path's native table block (and any header blocks the
   vendor made, which no readback shows), so that path may look better
   there than in any text readback. Nobody checked this
   by eye.
6. **Operator publishes are not label-prefixed.** `chat publish` posts the
   body as given, whereas bridge replies get the agent-label prefix. A reply
   whose body starts with `# ` therefore has that heading pushed onto the
   prefix line. Not exercised here.

## Follow-ups (not done)

- headings as real headings: the converted path posts Slack header blocks
  for `#` headings, and the vibe-talk shim maps header blocks back to
  Markdown headings, so a heading round-trips as a heading.

## Addendum: headings as headings (#551 headings-real-headings)

Two more posts in the same test thread, approved by the coordinator.

1. **Probe.** One app post with caller-supplied Block Kit blocks (header,
   mrkdwn section, header, rich_text) and a mrkdwn fallback. Read back:
   - `content` (all three formats) is the fallback text alone.
   - `rich_content` lists every block, header and section included
     (`header` / `plain_text` / `end_header`, and `section` / `section_text` /
     `mrkdwn` / …), with `rich_content_complete` true.
   - The vendor's own `--markdown` conversion, by contrast, leaves its header
     blocks out of `rich_content` (see the first run above).
   - The vibe-talk instance's then-current shim showed the fallback text.
2. **Headings check.** `corpus/01-report.md` by the new converted path, which
   sends headings as header blocks and the rest as sections, with today's
   mrkdwn as the fallback. Read back:
   - `rich_content` is complete: section (label line), header, section,
     header, section.
   - **The stored `content` has every newline turned into a space**: for a
     blocks post, the fallback comes back as one run-on line.
   - The new shim release rebuilds the message from the blocks. Its output,
     rendered by vibe-talk's renderer (Slack dialect), draws both headings as
     `<h2>`, with the lists, tables-as-code, quote and code intact.
   - The shim then running on the vibe-talk instance shows the run-on fallback
     instead. So the shim must switch before the bridge's helper does.

Not verified: how the official Slack client draws the header blocks, because
no one could check it by eye from this lane.
