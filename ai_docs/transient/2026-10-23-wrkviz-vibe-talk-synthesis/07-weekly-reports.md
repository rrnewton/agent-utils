# Appendix 7: Weekly project reports

> **Update 2026-10-23 (`#455 vibe-talk-next-plan`).** Part of the master plan,
> [`../2026-10-23-vibe-talk-next/README.md`](../2026-10-23-vibe-talk-next/README.md), which links
> here for the detail of weekly reports (milestone 0.7.0). Terms follow its
> [terminology](../2026-10-23-vibe-talk-next/TERMINOLOGY.md).

Part of `#449 wrkviz-vibe-talk-synthesis`. An automatic weekly draft that merges several channels,
and the transcripts behind them, into one product update. The owner's case: separate channels with
separate agent teams on different machines, and one update posted to an internal team feed that must
cover both.

## Inputs

| Input | Layer | Cost |
|---|---|---|
| Daily chat rollups per channel (technical) | chat | batch, cheap (appendix 6) |
| Daily technical rollups per transcript team | transcripts | batch |
| Owner prompts of the week, verbatim, from the prompt projection (`extract-transcripts`) | transcripts | deterministic |
| Landed commits and merged pull requests per repository, with titles (`github_metadata.py`, through `gh-paced`) | deterministic | free |
| Issues opened and closed, with slugs (minibeads files, or the forge's issues) | deterministic | free |
| Accepted workstream topics and their activity counts | both | deterministic once topics exist |
| Last week's approved report | owner's document | free |

Everything deterministic is gathered automatically every week. The model sees only summaries,
counts, titles and the owner's prompts, never raw transcripts, which keeps the input to a few tens of
thousands of tokens.

## Configuration

```toml
[[reports]]
name = "product-update"
sources = { channels = ["C0123", "C0456"], teams = ["build-box-a/claude/4f2c…", "build-box-b/codex/9a1e…"] }
repositories = ["owner/project", "owner/agent-utils"]
audience = "plain"                 # plain | technical | both
schedule = "Fri 15:00"             # local; the draft is ready by then
model = "sonnet-5.5"
template = "product-update"        # sections below
```

One report can merge channels from two deployments: the second deployment's daily rollups are
fetched as artifacts (they are content-addressed JSON, so copying is safe and verifiable), not
recomputed.

## Pipeline

1. **Gather** (deterministic, Thursday night): the inputs above for the half-open week in the
   report's timezone; a manifest of every artifact id and count used.
2. **Fill gaps** (batch): any daily rollup missing for a source-day is generated, within the batch
   budget; a day that cannot be summarized is listed as such, never silently skipped.
3. **Per-source weekly technical rollup**: wrkviz's existing `technical-rollup` at week level, one
   per channel and per team.
4. **Merged draft**: one call that receives the per-source weekly rollups, the deterministic facts,
   last week's report and the template, and returns structured output: sections, each bullet with
   the source ids it rests on.
5. **Check** (deterministic): every PR number, issue slug and count in the draft must appear in the
   gathered facts; a bullet whose citation does not resolve is flagged in the review view. Any
   claim of "landed" must have a landed commit behind it (wrkviz's plain-language rollup already
   forbids turning "approved" into "landed").
6. **Review**: the draft appears in the web app under Reports, beside last week's, with every bullet
   expandable to its evidence. The owner edits, approves, and copies or exports it. **Nothing is
   posted automatically.**

## Format

```markdown
# Product update, week of Oct 19 to Oct 23

## Highlights
- Voice catch-ups now answer from the full text of recent messages, within the voice service's
  time limit, and know which messages are yours. (#437 voice-full-text-catchup, #438
  voice-owner-identity)
- ...

## By workstream
### voice-context
- What changed, in plain language. Why it matters. [3 sources]
### message-cache
- ...

## Landed
- 41 commits across 2 repositories; 12 issues closed, 5 opened.

## Next week
- From open issues and the owner's prompts of the last two days, labelled as plans, not results.

## Risks and asks
- ...

<sub>Drafted from 2 channels and 2 agent teams; 5 of 5 days summarized for each.
Not shown: 1 channel-day with no messages.</sub>
```

The footer is the report's own availability statement, the same idea as the voice bar: it says what
was read and what was missing.

## Cost

About 30 k to 60 k input tokens for the merged draft plus the per-source weekly rollups; on
Sonnet 5.5 roughly $0.10 to $0.30 a week (estimate). Gap-filling daily rollups dominate if the
nightly batches were off.

## Later

- A **monthly** or **quarterly** report is the same pipeline over wrkviz's existing monthly and
  quarterly rollup levels.
- A **diff view**: what this week's draft says that last week's "Next week" promised, and what it
  dropped.
