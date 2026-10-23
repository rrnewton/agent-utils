# Appendix 8: Voice agent context, and the summary-availability bar

> **Update 2026-10-23 (`#455 vibe-talk-next-plan`).** Part of the master plan,
> [`../2026-10-23-vibe-talk-next/README.md`](../2026-10-23-vibe-talk-next/README.md), which links
> here for the detail of voice briefings and the availability bar (milestones 0.4.0 and 0.5.0).
> Terms follow its [terminology](../2026-10-23-vibe-talk-next/TERMINOLOGY.md). What is pushed at
> session start or at fully ready is a **briefing** in the plan's terms.

Part of `#449 wrkviz-vibe-talk-synthesis`. What the voice agent is handed when a call opens, what it
is handed once server-side summaries are fully ready, which of those are pushed and which it pulls,
how long each preparation takes, and the floating bar that shows the owner how much is loaded.

## Two moments

1. **Session start.** The call must open as fast as it does today: the page fetches the assembled
   prompt (`GET /api/v1/voice-agent` on a deployment-managed bridge) or opens the hosted agent's
   socket, and the greeting follows. Anything pushed here has to be ready in **about 100 ms of server
   time** and small enough not to slow the model's first turn (a budget of about **4 k tokens**,
   proposed).
2. **Fully ready.** Some seconds to a minute later, the on-demand summaries for the window the owner
   chose are caught up. The agent can now give a real overview.

## The candidate preparations, and how long each takes

Deterministic latencies were measured on 2026-10-23 with Python `sqlite3` on a synthetic store
shaped like vibe-talk's message cache (13,500 messages over 30 days, five channels, 450 messages on
the most recent day, 127 MB, FTS5 trigram; the script and output are in appendix 10). Rust over the same SQLite
will be no slower. Model latencies are **estimates** unless marked measured.

| # | Preparation | Kind | Latency | Size | Ready at |
|---|---|---|---|---|---|
| P1 | Availability status: messages per hour for the window, which hours are caught up, newest message per channel | deterministic | **0.6 ms** measured (48 h of hourly counts and watermarks, all channels) | ~100 tokens | start |
| P2 | Topic list: slugs and one-sentence descriptions of threads active in the window (from stored thread names), plus accepted workstreams | deterministic read of stored names | under 2 ms (estimate; same order as P1) | 20 to 40 threads a day, ~40 tokens each, ~1.5 k | start |
| P3 | Index of the newest messages: id, local time, author, first 160 characters (as `digest` does) | deterministic | **1.3 ms** measured for all 450 of a day (87 KB, ~22 k tokens, so the push takes only the newest ~15) | ~0.7 k for 15 lines | start |
| P4 | Summaries of closed hours that are already caught up (from earlier batches or calls) | deterministic read | ~1 ms (estimate) | ~150 tokens an hour; push at most 4 (~0.6 k) | start |
| P5 | Full text of the window | deterministic | **6 ms** measured for a day (665 KB, ~150 k tokens) | too big for a voice model; **never pushed**; this is research's input | start (pull only) |
| P6 | Missing hourly rollups for the window, per channel | model, on demand | **estimate** 2 to 5 s per call on a Haiku-class model with ~4 k input; a median day has ~40 active channel-hours; at 4 calls in flight, ~20 to 50 s cold. Warm (only the current hour missing): ~5 s | ~150 tokens per channel-hour | fully ready |
| P7 | A 24-hour narrative from the hourly rollups ("today, in three sentences per workstream") | model, on demand | **estimate** 4 to 10 s after P6 | ~400 to 800 tokens | fully ready |
| P8 | Workstream topics with today's status line each | model, batch | precomputed nightly or weekly; at call time a read (~1 ms) | ~40 tokens each | start, if a batch ran |
| P9 | Transcript-layer summaries (agent phases) | model, batch | **measured** 20 to 24 s median per phase job through a coding CLI (usage receipts); not for call time | — | never at call time |
| P10 | Today's upstream chat read of every channel (the call-start read-ahead, as today) | network | **measured** 1.9 to 5.7 s per channel when the adapter is quiet; 14 to 20 s and timeouts when busy (voice-tool audit) | — | warms the held copy; not pushed |

The start push adds up to about 3.3 k tokens (P1 0.1 k, P2 1.5 k, P3 0.7 k, P4 0.6 k, P8 about
0.4 k for ten workstreams), inside the proposed 4 k budget; the budget itself waits on the voice
model's measured context ceiling (appendix 9, task 2).

What this says: everything the start push needs (P1 to P4, P8) is **milliseconds** if it comes from
the store, and **tens of seconds** if it has to come from upstream reads (P10) or new model calls
(P6, P7). The store and the backfill walk (appendix 2) are what make a rich start push possible;
the model work is what makes "fully ready" a second moment.

Before any of these defaults are fixed, `logbook bench` (proposed) replays a recorded day through
P1 to P7 against the configured model and records a receipt per call, so P6 and P7 become measured
numbers on the real gateway.

## Push or pull, at each moment

| Content | At start | At fully ready | Always pullable by |
|---|---|---|---|
| P1 availability status | **push**, one line | **push** an updated line (part of the ready notice) | — |
| P2 topic list | **push** | push only the topics that changed | `topics` tool (new) |
| P3 newest-message index | **push** the newest ~15 lines | — | `recent_activity` (full text, exists) |
| P4 caught-up hour summaries | **push** up to 4 | — | `summaries` tool (new) |
| P6 hourly rollups | — | **pull**, announced: the ready notice says they exist | `summaries(hours, channel)` |
| P7 24-hour narrative | — | **push**, in the ready notice, at most ~800 tokens | `summaries(level="day")` |
| P5 full text | never | never | `recent_activity`, `read_page`, `read_message`; or ask the research agent |

The reasoning:

- **Push what makes the first answer right**: the shared vocabulary (topic slugs), how much is known,
  and the very newest messages. The Ask panel design and the voice-tool audit both trace the voice
  agent's worst answers to what it was handed, and a model that knows the slugs can say "the
  runner-wedged thread" the way the page does.
- **Pull what is large or narrow.** A voice model's context and tool reliability are the scarce
  resources (appendix 9 compares them), so hourly detail stays behind a tool.
- **Push the narrative at fully ready, briefly**, because the owner's typical first question is
  "what happened?", and a voice model asked that with a narrative in context answers in one turn
  instead of four tool calls.

### How each push travels

| Backend | At start | At fully ready |
|---|---|---|
| Deployment-managed bridge (`vibe-talk-v1`) | In the assembled prompt from `GET /api/v1/voice-agent`, a fenced "context as of" block after the rules | **Protocol gap**: the bridge accepts `replay` once and has no silent context frame. Two options: (a) add a `context` client frame, capability-gated like `replay` (a bridge that does not list `context` is never sent one), answered by `context_ack`, never opening a turn; or (b) send a `notice` turn (exists today for relayed messages), which the agent will answer aloud. Proposed: (a), with (b) as the fallback the owner can turn off |
| Hosted agent | Prompt override where the agent allows it, otherwise a note after the conversation starts (`voice_prompt::for_session`) | `contextual_update`, which is silent |

Both paths fence the content as untrusted text and mark the owner's messages, exactly as tool
results are marked today (`#438`).

## What the voice agent says while summaries are preparing

The start push carries a status line such as:

```
[summaries as of 14:32] Window: the last 1 day. Ready: the last 5 hours (43 messages), and the
thread list. Preparing: 12 earlier hours (7 more had no messages), expected within about a
minute. Until then, answer from recent_activity and say that fuller summaries are on the way.
```

The system prompt (a version 13 rule) tells the agent to begin answering at once from what it has,
to mention once, briefly, that the server is still preparing the earlier hours, and not to guess
about them. For example:

> **Owner:** What's been going on today?
>
> **Agent:** In the last five hours the main thing is the runner-wedged thread: the coding agent
> found a lock held across an await and has a fix through review. There's also activity in
> cache-key-rewrite. The server is still summarizing the earlier part of the day; I'll have that in
> about a minute. Want the details on the runner fix meanwhile?

When the ready push arrives (silently, by `context` or `contextual_update`), the agent offers it at
the next natural pause rather than interrupting:

> **Agent:** The earlier hours are ready now. Before noon it was mostly the message-cache work and a
> long gate run. Want that, or keep going on the runner fix?

If preparation fails (the model unreachable, the budget hit), the status line says so in plain words
and the agent says what it can and cannot cover ("I only have the last five hours summarized; the
summarizer could not be reached for the rest"), following the `#441 voice-honest-answers` rules.

## The summary-availability bar

A floating strip that appears with the voice view (and on demand in the other views), in the style
of the read-aloud strip (`#read-steer`, `#376`, `#433`): docked above the controls on a phone,
movable on a desk, dismissible. It states, live:

> in the last **[1]** day the last **[5]** hours are summarized, containing **[43]** messages

### Collapsed

```
┌──────────────────────────────────────────────────────────────────────┐
│  in the last ┌───┐ day  the last ┏━━━┓ hours are summarized,     ⋯  ✕ │
│              │ 1 │▾▴            ┃ 5 ┃                               │
│              └───┘              ┗━━━┛  containing ┏━━━━┓ messages    │
│                                                    ┃ 43 ┃            │
│                                                    ┗━━━━┛  ◌ 12 more │
└──────────────────────────────────────────────────────────────────────┘
   [1]  the window, set by the owner (stepper; 1 to 7 days); plain border
   [5]  consecutive caught-up hours counting back from now; max 24 × days
   [43] messages inside those hours
   ◌ 12 more   hours still being summarized (spinner); absent when nothing is pending
```

Border colours (drawn as the box outline, with a text equivalent for screen readers):

| State | `hours` box | `messages` box |
|---|---|---|
| Every hour of the window is caught up | **green** | **green** |
| Some hours caught up, more preparing | neutral | neutral |
| An hour inside the counted run fell behind (an edit, a late message) | amber, and the count drops to the hours after it | amber |
| Nothing is summarized and nothing is preparing (no model configured, budget hit) | neutral, with the reason in the detail | neutral |

The day box never turns green: it is a setting, not a measurement.

### Expanded (tap ⋯)

The same moment as above, 14:32: the last five hours (10:00 to now) are caught up; twelve hours are
being summarized; seven overnight hours had no messages.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  in the last [1] day the last [5] hours are summarized, containing [43]      │
│  messages                                                              ⌃  ✕ │
│                                                                              │
│  15 16 17 18 19 20 21 22 23 00 01 02 03 04 05 06 07 08 09 10 11 12 13 14     │
│  ◌  ◌  ◌  ◌  ◌  ◌  ·  ·  ·  ·  ·  ·  ·  ◌  ◌  ◌  ◌  ◌  ◌  ■  ■  ■  ■  ■      │
│                                                                              │
│  ■ caught up   ▣ behind   ◌ being summarized   · no messages   □ not asked   │
│                                                                              │
│  build-lane    last 5 h caught up    31 msgs   ●                             │
│  infra         last 5 h caught up    12 msgs   ●                             │
│  research      no messages in the window       ●                             │
│                                                                              │
│  Model: Haiku 5.5   Today: 214 k tokens, about $0.03         [Summarize all] │
└──────────────────────────────────────────────────────────────────────────────┘
```

Once the twelve hours land, the empty hours count as caught up too, so the run reaches all 24
hours and both boxes turn green.

### The numbers, precisely

- **D** (days) is the owner's setting, kept per device; raising it to 2 raises the hours maximum to
  48 and queues the newly included hours for on-demand summarization (with an estimate when it is
  more than a few cents).
- **H** (hours) is the length of the run of consecutive hours, counting back from the current hour,
  whose freshness is `CaughtUp` (appendix 2). An hour with no messages counts as caught up: there is
  nothing to summarize. An hour that is `Unproven` (the store cannot yet prove it has every message)
  stops the run, because green must mean complete.
- **The current hour** would otherwise flicker: a message arriving makes it `Behind` until it is
  re-summarized. It is re-summarized at most once a minute while the bar is open, and a current hour
  whose only unsummarized messages are under 60 seconds old still counts. The expanded strip shows
  it as behind meanwhile, so nothing is hidden.
- **M** (messages) is the number of non-noise messages in the H hours, across the channels in scope.
- All three update live from server-sent events: a message arriving, a summary landing, an hour
  falling behind. The bar reads only tallies and freshness (P1), never message text.

### Scope

The bar's scope is the voice agent's scope: every channel by default, or the channel on screen when
the call is about one channel. The expanded view lists channels so the owner can see which one is
lagging.
