# Appendix 9: A server-side research agent the voice agent hands questions to

> **Update 2026-10-23 (`#455 vibe-talk-next-plan`).** Part of the master plan,
> [`../2026-10-23-vibe-talk-next/README.md`](../2026-10-23-vibe-talk-next/README.md), which links
> here for the detail of deep research and the voice hand-off (milestone 0.5.0). Terms follow its
> [terminology](../2026-10-23-vibe-talk-next/TERMINOLOGY.md). The "helper mode" and "research mode"
> below are research's quick and deep **depths**, and one research log in the Ask panel holds the
> answers to questions asked in the panel and by voice.

Part of `#449 wrkviz-vibe-talk-synthesis`. Added at the owner's request on 2026-10-23:

> One potential architecture is that we could develop our smarter server-side side agent as our kind
> of research helper. Maybe when it wants to deep dive on a topic, it has the tools to read both the
> chat history and possibly raw agent and sub-agent history. Of course that would be pull-based
> random access, not pushed to the agent. Maybe we could accept the voice agent is going to be kind
> of dumb, and we could have particular phrasing that it uses to pass a question to the research
> agent. If I say "research for me X", it would have the ability to make an API call to send it to
> the server-side research agent, which could dig deeper before getting back.

## The split, in one paragraph

The voice model is good at conversation and bad at precision over large text; a server-side model is
the reverse. So the voice agent stays a fast, honest front end with a small, push-fed picture
(appendix 8), and anything that needs digging is **handed off as an asynchronous job** to a
server-side research agent that has random-access tools over the chat history, the raw agent and
subagent transcripts, and the store's search. The answer comes back as a short spoken summary plus a
written answer with citations on the page.

## The research agent's tools

All read-only, all through `logbook` with a `Capability` (appendix 2), all returning fenced,
owner-marked text with ids the server can validate.

**Whose capability.** A voice call holds the read token, which reaches transcript summaries at most
(appendix 5). A research job therefore runs under a **server-held research capability** that the
owner configures, not the caller's token, with two limits on that escalation:

- `research.from_voice` sets what a job started from a voice call may read: `summaries` by default,
  or `events` if the owner opts in. Payloads are never reachable from a voice-started job.
- What flows back is limited by where it goes: the voice call receives only the short `spoken`
  answer; the `written` answer with transcript citations is shown only to a page holding the write
  token. A job started from that page may use the deployment's full transcript level.

| Tool | What it reads | Notes |
|---|---|---|
| `list_topics(span?)` | Thread, term and workstream topics with sentences and activity | The vocabulary; usually its first call |
| `get_summaries(level, span, sources)` | Hourly, daily, weekly rollups, thread sentences, phase and agent summaries | Cheap orientation before reading raw text |
| `search_chat(query, channels?, span?, authors?)` | FTS over chat (ANDed words, quoted phrases) | Hits carry message id, thread slug, time, author |
| `read_thread(channel, thread)` / `read_messages(channel, span or ids)` | Full chat text | Paged; the server caps a page at ~30 k tokens |
| `search_transcripts(query, teams?, kinds?, span?)` | FTS over prompts, responses, inter-agent messages, tool names | Requires transcript capability `events` |
| `list_agents(team, span)` | Agents with names, roles, lifetimes, parents | From the groups table |
| `read_phase(phase)` | One phase's summary, and with `events` its transcript | wrkviz's `show phase:… --transcript` |
| `read_events(agent, span, kinds, limit)` | Raw normalized events of one agent | Subagents included |
| `read_tool_payload(call)` | A tool call's input or output, redacted | Requires `payloads`; off by default |
| `owner_prompts(span, teams?)` | The owner's own prompts, verbatim | From the prompt projection |
| `who_posted(message)` | The agent phase that posted a chat message | The posting join (appendix 2); absent when unmatched |
| `landed(span, repos?)` | Commits and pull requests that landed, with titles | Deterministic facts for "did X actually land?" |

It has **no write tools**: it cannot post, mark read, run commands or reach the network. Its outputs
are an answer and citations, as quick research's are.

### Making pull-based access safe from "only part of the picture"

The owner rejected tool-pull for quick research (then the side-agent helper) because a model choosing what to read can miss
things. For research, pull is unavoidable (the history is too big to push), so three mitigations:

1. **Seeded with a push.** Every research job starts with the briefing quick research uses for the scope: the
   topic list, the thread index and the newest-message index (appendix 8, P2 to P4). The agent knows
   what exists before it searches.
2. **Coverage is part of the answer.** The structured result must say what was searched ("chat:
   build-lane and infra, 7 days, 3 searches, 4 threads read; transcripts: coordinator and 2
   subagents, Oct 20 to 23"). The server checks the claim against the job's tool log and corrects it.
3. **A budget it must spend sensibly**: at most 25 tool calls and a wall-clock cap; on exhaustion it
   answers with what it has and says what it did not reach.

## How the voice agent hands a question off

### The trigger

A version-13 prompt rule:

- When the user says **"research …"**, "research for me …", "dig into …", or "look into … properly",
  call `research` with their question **verbatim** (plus the channel, if one is in play), say the
  acknowledgement the tool returns, and carry on with the conversation.
- When `recent_activity` does not hold the answer, the agent may **offer** research ("Want me to send
  that to research?") and calls it only on a yes.
- Never claim to have researched something itself.

### The tool

```
research(question: string, channel_id?: string, depth?: "quick" | "deep") ->
  { job_id, status: "started", expected_seconds, say: "..." }      within 1 s
research_status(job_id) -> { status, steps_done, last_step, elapsed_seconds, say }
research_cancel(job_id) -> { status: "cancelled" }
```

`research` returns at once (well inside the runtime's 15 s tool limit) with a sentence to speak:
"I've sent that to the research assistant. Quick questions take about half a minute; I'll tell you
when it's back." The job runs on the server whether or not the call continues.

### While it runs

> **Superseded in part, 2026-10-23 (`#455 vibe-talk-next-plan`).** The master plan keeps the
> research card in the Voice view for a voice-started job, and also files every answer, with its
> written part and citations, in the Ask panel's research log, so there is one history of research
> answers.

- **On the page**: a research card in the voice transcript, updated by server-sent events, listing
  steps as they happen ("searched chat for 'runner lock': 12 hits", "read thread runner-wedged",
  "opened the coordinator's 10:42 phase"), with Cancel.
- **By voice**: nothing unprompted until it finishes, except one status line if a deep job passes
  60 s ("Research is still going; it has read four threads so far"). If the owner asks "how's the
  research going?", the agent calls `research_status`.

### How the answer comes back and is spoken

```json
{
  "spoken": "Two to four plain sentences, no ids, no markdown, at most about 600 characters.",
  "written": "A fuller answer in Markdown with [1] [2] markers.",
  "citations": [{"n": 1, "kind": "chat", "channel": "…", "message_id": "…", "label": "runner-wedged, 10:58"},
                {"n": 2, "kind": "phase", "ref": "phase:team::…", "label": "coordinator, 10:42 to 11:12"}],
  "coverage": "What was searched and read, as checked by the server.",
  "answerable": true
}
```

- Every citation is validated against the job's tool log; an unverifiable one is dropped and the
  answer says so.
- **During a call**: the result is pushed silently (the `context` frame proposed in appendix 8, or
  `contextual_update` on the hosted backend) with the instruction to offer it at the next pause:
  "The research on the runner lock is back. Want it now?" On a yes the agent speaks `spoken`. The
  page shows `written` with tappable citations (a chat citation jumps to the message; a phase
  citation opens the Work tab at that phase).
- **After the call ended**: the result is kept; the page shows a badge; the next call's start push
  includes "Research results waiting: the runner lock (finished 14:05)".
- It is never posted to a channel. A draft reply, if wanted, goes through the Ask panel's draft action
  and the owner's Send.

### Latency budget

| Step | Budget |
|---|---|
| `research` acknowledgement | under 1 s (a job record and a queue push) |
| Quick job (a few searches and reads, one model loop on a fast model) | target 15 to 30 s; cap 60 s (estimate) |
| Deep job (transcripts included) | target 1 to 2 min; cap 3 min (estimate) |
| Each model step, Sonnet-class, warm prompt cache | 2 to 8 s (estimate) |
| Each tool call against the local store | milliseconds (appendix 8 measurements); remote transcripts add the transport's round trip |

## Relation to quick research (the push-fed side-agent helper)

| | Quick depth, the Ask panel (`2026-10-22-side-agent-helper.md`) | Deep depth (this appendix) |
|---|---|---|
| Purpose | Completeness over a bounded window: "where was the answer to X?" | Depth over unbounded history: "why did X happen, and did it land?" |
| Context | Pushed: every thread, full text of the last N hours, appended as messages arrive | Pulled with tools, seeded with a briefing |
| Turn shape | One model call per question, warm prompt cache, seconds | A tool-using loop, tens of seconds to minutes, asynchronous |
| Reaches transcripts | No | Yes, by capability level |
| Outputs | Answer, citations, highlight, draft | Answer, citations, coverage |

**One service, two depths** (first written as "same brain, two modes"). Both are one server-side
research service: the Messages API backend, the briefing builder, the fence, the owner marking, citation validation and structured output. The quick
depth keeps a push-fed transcript; the deep depth runs a tool loop. Quick can escalate ("this
needs the transcripts; research it?"), and the Ask panel proposal's phase 3 `ask_helper` became
`research(depth: "quick")`, answered from the panel's warm session when one exists. Building the
core once and the two modes on it is less code than two agents and keeps their rules identical.

## The voice model versus a server-side model

What is known from this repository and the voice-tool audit, and what is not:

| | Voice LLM (private stack, deployment-managed) | Voice LLM (hosted agent) | Server-side Haiku 5.5 | Server-side Sonnet 5.5 |
|---|---|---|---|---|
| Model | Not identified in this repository | Chosen in the vendor console | Haiku 5.5 | Sonnet 5.5 |
| Context window | **Unknown**; observed handling a +16,616-token tool result | Vendor-dependent | 1 M | 1 M |
| Tool calling | Runtime limits logged: `max_iter=5`, `timeout=30s`, `tool_timeout=15s`; calls of one turn run **sequentially** | Vendor-managed; per-tool timeouts | Reliable parallel tool use; no runtime cap but ours | Same; better at multi-step plans and precise citation |
| Observed failures | Invented abilities and causes, used a runtime web search for a chat question, read ids and tool names aloud (prompt v10); TTS runaway on technical sentences (synthesizer, not the LLM) | — | Unmeasured here | Unmeasured here |
| Latency | Must answer in conversational time: first audio within about a second; a tool round trip is a pause the owner hears | Same | 1 to 5 s per step (estimate) | 2 to 8 s per step (estimate) |
| Cost | Self-hosted: fixed cost, no per-token bill | Per minute of conversation | ~$0.002 per warm question on a 100 k briefing (`2026-10-22-side-agent-helper.md`) | ~$0.025 per warm question; a deep research job of ~300 k input tokens with caching ~$0.10 to $0.40 (estimate) |
| Best use | Talking, orientation from a small pushed picture, deciding to delegate | Same | Topic suggestions, hourly rollups, quick research | Deep research, weekly reports, phase summaries where precision matters |

The asymmetry is the reason for the design: the voice side's context and tool loop are the scarce,
unmeasured resources, and the server side's are plentiful and measurable.

## Measuring the voice model's abilities

A capability card per voice model, measured by a repeatable harness rather than inferred from
incidents.

**Harness.** Two drivers. For the hosted backend, reuse `vibe-talk/scripts/smoke-agent.py`, which
already drives a real agent headlessly by text (`user_message`) and checks this server's access log
for tool calls; pointing that agent at a test server needs a second agent (or a changed MCP URL) in
the vendor console. For the deployment-managed stack, whose limits are the unknown ones, **a new
driver** is needed: a small `vibe-talk-v1` client that opens a session, sends typed `prompt` frames
and reads `turn_complete` and the transcript events. Both run against a vibe-talk **test** server
backed by the fake chat provider and a fixture corpus with known answers (a synthetic day built by
the ported `synthetic.py`, plus a recorded, redacted real day). It costs voice minutes and is opt-in,
as the smoke script is today; running it against the production stack needs the owner's go-ahead.

**Tasks** (each a few dozen fixtures, scored automatically where possible and by a server-side judge
model with spot checks otherwise):

| # | Task | Measures |
|---|---|---|
| 1 | Catch-up fidelity: questions whose answers sit at different depths of a `recent_activity` result | Precision over a tool result, by its size (2 k, 8 k, 16 k, 32 k tokens) |
| 2 | Context ceiling: push 2 k to 64 k tokens of context at start, then ask about early, middle and late items | Usable context, first-turn latency by push size; where it breaks |
| 3 | Tool choice: 30 phrasings mapped to the right tool and arguments | Tool-selection accuracy |
| 4 | Multi-step: find, then read, then answer | Success within `max_iter`; wasted calls |
| 5 | Owner rule: windows full of the owner's messages | Never reads the owner's messages back |
| 6 | Speakable output: ids, flags, paths, markdown in sources | Violations per answer |
| 7 | Honesty: questions outside its abilities (logs, builds, machines) | Refuses and says why; invented abilities |
| 8 | Research handoff: 40 utterances, half of which should trigger `research` | Precision and recall of the trigger; the question passed verbatim |
| 9 | Ready-notice handling: a silent context push mid-conversation | Offers it at a pause; does not interrupt or ignore |
| 10 | Latency: per turn, per tool round trip, by context size | Seconds to first audio |

**Outputs.** A table per model and prompt version, kept under `vibe-talk/tests/voice-eval/` results
(numbers only, no message text from real channels), re-run on every prompt version bump, model change
or runtime change, as a regression check. The same fixtures run against Haiku 5.5 and Sonnet 5.5 in
text form give the server-side baseline, so the comparison table above can be filled in with
measurements instead of "unknown".

The first numbers to get are tasks 2 and 8: the push budget for appendix 8 depends on 2, and the
whole hand-off design depends on 8.
