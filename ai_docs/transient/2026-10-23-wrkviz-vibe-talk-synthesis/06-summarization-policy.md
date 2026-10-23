# Appendix 6: Summarization policy and costs

Part of `#449 wrkviz-vibe-talk-synthesis`. The owner's rule: **deterministic work is automatic;
work that spends tokens runs on demand or in configured batches.** This appendix says which step is
which, what each costs on real volumes, and the controls.

## Volumes

| Layer | Volume | Source |
|---|---|---|
| Chat, busiest deployment | 62 to 446 message events a day across five channels (median about 200); about 330 tokens a message; about 70 k tokens on a median day, 150 k on a heavy one | measured 2026-10-22, side-agent doc |
| Agent transcripts, one owner's 12 teams | 499,155 events in 60 days (about 8,300 a day); 18,371 phases (about 306 a day); 2,912 agents (about 48 a day) | measured 2026-10-23 from the archive bootstrap |
| Paid transcript summaries so far | 2,425 phase/rollup jobs at 17.9 k input and 1.6 k output tokens each on average; 640 naming jobs at 13.8 k input and 0.4 k output | measured from usage receipts |
| Raw transcript bytes on this host | Files touched in 24 h: Claude 2.46 GB (39 files), Codex 5.69 GB (126 files); whole-file parse 16.6 s and 24.9 s | measured 2026-10-23 |

## Which step is which

| Step | Kind | When it runs |
|---|---|---|
| Chat live feed into the store; transcript polls by cursor; backfill walk of chat history | deterministic | **automatic**: feed continuously, transcripts every minute, backfill hourly per channel |
| Indexing (FTS), tallies per hour, activity bins, watermarks, freshness, the exact chat-to-transcript join | deterministic | **automatic**, after each apply |
| Thread topics (from thread names) and term topics (issue slugs) | deterministic | **automatic** |
| Timeline export per channel; archive build | deterministic | **automatic** every few minutes while anyone is watching, otherwise hourly |
| Weekly report data gathering (counts, landed commits, closed issues) | deterministic | **automatic**, weekly |
| Per-message one-liners for long messages | model | **on demand**, as today (the page asks when it shows a long message) |
| Thread names and sentences | model | **automatic today**, Haiku, budgeted per channel. Proposed: keep, under the daily ceiling below; it is the cheapest model spend and what every other level names things by |
| Chat hourly rollup (per channel) | model | **on demand** for the window the owner opens (voice call, availability bar, Work tab); **batch** nightly for hours nobody opened, if configured |
| Chat daily rollup | model | **batch**, nightly; **on demand** when a report or a two-day window needs it |
| Transcript phase summaries and agent names | model | **batch**, nightly for the previous day's active teams within a budget; **on demand** from "Summarize this view" |
| Transcript hourly and daily technical rollups | model | **batch**, nightly |
| Plain-language rollups and weekly reports | model | **batch**, weekly, plus **on demand** "draft now" |
| Workstream topic suggestions | model | **batch**, weekly |
| Research agent and side-agent helper | model | **on demand** only |

## Costs

List prices per million tokens, first-party, as tabulated in the side-agent doc; the deployments
reach models through a gateway whose prices may differ. All figures in this section are estimates
built from the measured volumes above, not measured bills.

| Model | Input | Output |
|---|---|---|
| Claude Haiku 4.5 (vibe-talk's default today) | $1 | $5 |
| Claude Haiku 5.5 | $0.10 (to 100 k context) | $0.50 |
| Claude Sonnet 5.5 | $2 | $10 |

### Chat layer

Assume a median day: about 40 active channel-hours across five channels, and an hourly rollup
input of that hour's full text plus about 1.5 k tokens of instructions, the previous hour's summary
and the thread index.

| Job | Per day | Haiku 4.5 | Haiku 5.5 | Sonnet 5.5 |
|---|---|---|---|---|
| Hourly rollups, all active channel-hours | ~130 k in, ~12 k out | ~$0.19 | ~$0.02 | ~$0.38 |
| Daily rollups, five channels | ~40 k in, ~5 k out | ~$0.07 | <$0.01 | ~$0.13 |
| On demand only (the owner opens one 24 h window a day) | ~half of the hourly row | ~$0.10 | ~$0.01 | ~$0.19 |

The chat layer is cheap at any model. The reason to keep it on demand is not money but noise: an
hour nobody asks about does not need prose.

### Transcript layer

Full coverage at the measured averages: about 306 phase jobs a day at 17.9 k in and 1.6 k out, plus
about 48 naming jobs at 13.8 k in.

| Job | Per day | Haiku 5.5 | Sonnet 5.5 |
|---|---|---|---|
| Every phase summarized | ~5.5 M in, ~0.49 M out | ~$0.80 | ~$16 |
| Every agent named | ~0.66 M in, ~0.02 M out | ~$0.08 | ~$1.5 |
| Hourly and daily technical rollups, 12 teams | ~1 M in, ~0.1 M out | ~$0.15 | ~$3 |

So full transcript coverage is **about $1 a day on a Haiku-class model or about $20 a day on
Sonnet-class**, and the archive so far covers 10 % of phases. The input side dominates because each
phase job carries up to 16,000 characters of ancestor context (`ARCHITECTURE.md`, "Phase work
summary"). Two levers before choosing a model:

1. **Summarize only what someone will look at.** Phases of agents whose chat output the owner read,
   or that a report covers; everything else stays transcript-and-statistics until asked.
2. **Prompt caching of the ancestor context.** Consecutive phases of one agent share most of it;
   with the Messages API backend (not the CLIs) the shared prefix is billed at the cache-read price.
   Estimated saving 40 to 60 % of input on long-running agents.

Quality on Haiku 5.5 for phase summaries is unmeasured. wrkviz already has a blind model-comparison
plan (`py/wrkviz/ai_docs/MODEL_COMPARISON_EXPERIMENT.md`); running it on Haiku 5.5 against the
existing gpt-5.5 and Sonnet 5 artifacts is the first measurement to make.

## Controls

- **A daily token ceiling per deployment**, split into automatic (thread names), batch and on-demand
  shares. A batch that would exceed its share stops at the ceiling and says how far it got.
- **An estimate before every on-demand batch** larger than a few cents ("Summarize this view: 37
  phases, about 0.7 M tokens, about $0.08 on Haiku 5.5").
- **Receipts for everything**, chat included (today only wrkviz has them), and a spend line in
  Settings and in the availability bar's detail view.
- **Batch windows** in configuration, for example nightly at 02:00 local, weekly on Friday
  afternoon before the report.
- **No silent fallback** to another model or backend on failure, as wrkviz requires; the failure is
  recorded and shown.
