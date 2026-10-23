# Quick research and the Ask panel: a text agent that has read the chat, beside it

> **Update 2026-10-23 (`#455 vibe-talk-next-plan`).** This proposal is now part of the master
> plan, [`2026-10-23-vibe-talk-next/README.md`](2026-10-23-vibe-talk-next/README.md), as milestone
> 0.2.0 (Ask). Terms were changed in place to the plan's
> [terminology](2026-10-23-vibe-talk-next/TERMINOLOGY.md): the "side-agent helper" is **quick
> research**, its UI is the **Ask panel**, the context it is handed is a **briefing**, and
> `[helper]`, `/api/v1/helper/ask` and `ask_helper` became `[research]`, `/api/v1/research` and
> `research(depth: "quick")`. The owner's own words are quoted as they were said. The phased plan
> and the questions are superseded where marked; the rest stands as written on 2026-10-22.

2026-10-22. Design proposal for the slug `side-agent-helper` (its first name); no code. This is a
working note for the owner to decide on, and the questions are at the end.

## What the owner asked for

With coding agents writing faster than he can read, he asks for something, misses the answer and
cannot find it again. He wants a small helper agent behind an icon at the top of the page (upper
left, near the search glass). It opens as an overlay on a desk and as something else on a phone. It
holds a short transcript of its own with him, and it has read the full history of the chat, so it
can answer "where was the answer to X?" and questions about what the messages say.

Two things he has been clear about:

1. **Push-fed, not tool-pull.** "Feed in new messages, either in bulk or as we get them ... rather
   than letting the agent decide to pull messages with some kind of read tool interface. I am afraid
   there are too many mistakes it could make to get only PART of the picture."
2. **Two UI powers only.** It can point at a message, so the page scrolls to it and highlights it.
   It can also say that a question is not answerable from what is there and put a suggested prompt
   into a text box for him to edit, send or delete.

His other stated preferences:
- **Off by default.** It is started on demand, to save tokens, and shows that it is catching up.
- **A lit icon while it is active.** Then it receives new messages; it can be turned off again.
- **Bulk catch-up when turned back on**, covering everything it missed.
- **Grouped by thread** rather than in pure time order, with the thread names and summaries the UI
  already shows, so it talks in his terminology.
- **Never replies to fed messages.** Fed messages only keep it up to date; it answers only his
  questions in its own transcript.
- **An API, SDK or CLI rather than a terminal UI.** He mentions Claude, Codex and the other coding
  CLIs as hosts. (Settled by the owner on 2026-10-23: the API.)

The voice agent is the same idea by ear, and today it is, in his words, "profoundly stupid". The
2026-10-22 audit (`2026-10-22-voice-tool-audit.md`) found most of that came from what it was handed:
160-character lines, a tool with no deadline, no idea whose messages were his, and no rule against
inventing abilities. That audit is the cautionary tale this design is built around.

## What vibe-talk already has

| Piece | Where | Reusable for quick research |
|---|---|---|
| **An LLM client.** `[summaries.llm]` makes one plain Messages API call per summary (Anthropic or Vertex dialect, gateway headers, client-certificate auth, per-minute pacing), default `claude-haiku-4-5`. | `src/summarize/messages_api.rs`, `config.rs` `LlmConfig` | Yes. The transport, authentication and rate limiter carry over as they are. A `[research.llm]` table with the same shape can name a different model. |
| **Thread names and sentences.** Each thread gets a two-to-four-word name, chosen once and then kept, plus one sentence refreshed at 1, 2, 4 and 8 messages and then every 10. They are stored in the `summaries` table. | `src/summarize/threads.rs` | Yes. These are the names the owner sees, and research should use exactly them. |
| **Message text for a model**, fenced as untrusted, with the owner's messages marked "the user (Name)" (`#438`) and continuation lines indented so text cannot forge a line. | `src/untrusted.rs` `model_line`, `src/owner.rs` | Yes, verbatim. |
| **A held copy of each channel's recent window**, fed by live events, read within a deadline. | `src/recent.rs` | Partly. It holds only about 50 messages per channel. |
| **A disposable on-disk message cache** (`#208`, `#384`). It is filled from reads and live events, and served on request. | `src/cache/` | Yes. It is the right source for a bulk catch-up, but it is **off** on the busiest deployment today (see Prerequisites). |
| **MCP tools** for the voice agent: `recent_activity` (full text since `#437`), `read_page`, `find_message`, `read_message`, `count_messages`, `post_reply`. | `src/mcp/` | Not as quick research's interface, by the owner's choice. The rendering behind them is shared. |
| **Jump-to-message in the page.** `revealRow`, `jumpToReply`, bookmarks, and loading older pages to reach a target. | `web/voice.js` | Yes. A highlight action is a jump with a flash. |
| **The composer**, `#compose-text`, per channel. | `web/voice.html` | Yes, as the target of a draft action. |

## Measured volume

Read-only, from the busiest deployment's logs and store on 2026-10-22.

- **Live message events:** 62 to 446 per day across its five channels over the last ten days (median
  about 200). That count includes edits.
- **Threads named so far:** 339. The busiest channel has 239, the others 80 and 20.
- **Size of one message:** a 50-message page of the busiest channel came to about 16.6 k tokens in
  the voice model's own accounting, so about 330 tokens per message (a different tokenizer, so
  ±30 %). One coding-agent message is often 1–4 KB.
- **So a day of all channels is about 70 k tokens on a median day and 150 k on a heavy one.** A week
  is 0.5 to 1 M tokens.

**Full history every question is therefore not possible beyond a day or two**, even at 1 M context.
"Full picture" has to mean: every thread named and described, plus the full text of a bounded
recent span, with that bound stated to the model and to the owner.

## Models and access

### Which model

| Model (list price, per million tokens) | Input | Output | Cache read | Context | Fit |
|---|---|---|---|---|---|
| Claude Haiku 4.5, today's summariser | $1 | $5 | $0.10 | 200 k | Cheap, but 200 k is barely a heavy day. Fine for thread names; weak for "where exactly was X decided". |
| Claude Haiku 5.5 | $0.10 ($0.50 past 100 k) | $0.50 ($2.50) | about 0.1× | 1 M | Very cheap with a long context. Worth an evaluation as the default. |
| Claude Sonnet 5.5 | $2 | $10 | $0.20 | 1 M | The quality step. Precise citation over 100 k+ tokens of agent prose is where a stronger model pays. |
| Claude Opus 5.5 | $4 | $20 | $0.20 | 1 M | Overkill for routine questions; a per-question "think harder" escalation at most. |

Prices are first-party list prices. The deployment reaches models through a gateway, whose prices
may differ.

### Cost of a briefing and of one question

This is for a briefing of 100 k tokens (about a median day plus thread index), with prompt caching:
the briefing is written once, then read at the cache price.

| | Haiku 4.5 | Haiku 5.5 | Sonnet 5.5 |
|---|---|---|---|
| Briefing: write 100 k to the cache (1.25×) | $0.13 | ~$0.02 | $0.25 |
| One question, cache warm (read 100 k, about 500 output tokens) | ~$0.013 | ~$0.002 | ~$0.025 |
| One question, cache cold (no caching) | ~$0.10 | ~$0.01 | ~$0.21 |
| A day of 30 questions in 3 sittings | ~$0.8 | ~$0.1 | ~$1.5 |

The cache lives 5 minutes, or 1 hour at a 2× write price. Questions are bursty, so a sitting of
questions stays warm and a cold re-write happens about once per sitting. That is the line that
dominates the bill, and it is the reason the design below never calls the model just to "feed" it.

### API/SDK versus a coding-agent CLI

| | Messages API from the server (recommended) | A coding-agent CLI or agent SDK session (Claude Code, Codex and the like, under agentctl) |
|---|---|---|
| Who builds the context | The server, deterministically: every thread, in his terms, within a stated budget | The harness and the model. It will want to read files and run tools; feeding it chat means pasting into a terminal session |
| Tools the model holds | None. Its two UI actions are structured output the server validates | Shell, files and web by default. Message text is third-party text, so that is a prompt-injection surface onto a workstation |
| Push-fed, "don't reply until asked" | Native. The server holds the transcript and sends it only with a question | Each fed batch is a turn the harness answers (or must be told not to), costing a model call per batch |
| Latency and cost | One call per question; caching applies | Harness overhead per turn, larger system prompts, less control over caching |
| Reuse | `LlmConfig`, the pacing and the fence | agentctl for lifecycle |

A CLI session is the right tool when research needs to do work: open a log, run a build. The
owner's request is reading. The API path is narrower, cheaper and safer, and it is the one his
"push-fed" preference implies.

## Context strategy

There are three shapes:

1. **Stateless: rebuild the briefing for every question.** It is simple and always current. Without
   caching it pays the whole briefing on every question; with caching it costs about the same as (2)
   if the briefing is built append-only.
2. **A persistent session fed incremental messages, told not to reply until asked.** This is the
   owner's model. The important point is that it **needs no model call per fed batch**: a Messages
   API conversation is just the transcript the server sends. "Feeding" means the server appends the
   batch to the transcript it holds; the model sees all of it, in order, at the next question. The
   "don't reply" rule is still stated in the system prompt, because the model will see fed batches
   between his questions.
3. **Retrieval:** the model, or the server, picks what to include per question. The owner rejects
   model-driven pulling, rightly: it is how the voice agent got "only part of the picture". The
   server could still do retrieval as a fallback for history beyond the budget, labelled as such.

**Proposal: (2) implemented as an append-only transcript, which also gives (1)'s freshness.**

```
system:   research rules (below), frozen                               <- cache breakpoint 1
user:     [BRIEFING as of 13:02] thread index: every thread in scope,
          name + sentence + last activity + message count;
          then full text grouped by thread, newest-active thread last  <- cache breakpoint 2
user:     [QUESTION] where did we decide the overlay fix?
assistant:{answer, citations, actions}
user:     [NEW MESSAGES 13:05-13:20] grouped by thread (only threads that moved),
          each group headed by its name and current sentence
user:     [QUESTION] ...                                               <- automatic tail caching
```

- **Grouping.** One block per thread, oldest message first inside it. Main-channel messages are a
  "main" group per channel. Every group header carries the UI's own name and sentence, so the
  answer says "in runner-wedged" the way the page does. The owner's messages stay in, marked as the
  user's.
- **Strictly append-only.** Nothing earlier is rewritten. An edit or deletion upstream is appended
  as a correction line ("message X in runner-wedged was edited: ..."), never patched in place, so
  the cached prefix survives.
- **Budget.** It catches up the last N hours (default 24) in full text, plus the index of every
  thread back to D days (default 7). The briefing header says both bounds in words. When the
  transcript passes the model's comfortable size (say 60 % of context), the server starts a new
  session with a fresh briefing rather than letting the API compact it: compaction would summarise
  away exactly the text quick research exists to quote.
- **Thread sentences change.** A sentence refreshed since the briefing is re-stated in the next
  NEW MESSAGES group header for that thread, never edited into the old header.

## On-demand start and "catching up"

- **Off by default.** The icon is dim and nothing is spent.
- **Tapping it activates quick research.** The server builds the briefing from the message cache and
  the stored thread names, and the overlay says "Catching up on 4 channels: 212 messages in 31
  threads since yesterday 13:00". The server sends the briefing with `max_tokens: 0` to warm the cache
  and to count tokens. When that returns, the overlay says ready, giving the span and the token
  count, with a cost estimate if the owner wants it shown.
- **Active means the icon is lit.** New messages arriving over the live feed are appended to the
  research session's transcript as they come. That costs nothing until the next question.
- **Deactivating** (tap the lit icon, or idle for an hour, as the owner chooses) drops the server
  transcript. Turning it back on is a fresh briefing, which covers "everything it missed" without a
  separate backlog path.
- **Its own transcript with him** is kept in the store like voice conversations (`turns`), so a
  reload or a second device shows it.

## The UI-control protocol

Research answers in **structured output** (JSON schema, validated), never free text with embedded
commands:

```json
{
  "answer": "plain text, with [1] [2] markers",
  "citations": [{"n": 1, "message_id": "...", "channel_id": "...", "quote": "first words..."}],
  "actions": [
    {"type": "highlight", "message_id": "...", "channel_id": "..."},
    {"type": "draft", "channel_id": "...", "reply_to": "...", "text": "...", "why": "..."}
  ],
  "answerable": true
}
```

- **The server validates every id against the messages it fed this session.** A citation or
  highlight naming anything else is dropped, and the answer says one of its references could not be
  checked. Ids come from the line brackets the model was given, the same handles the voice tools
  use.
- **`highlight`.** The page jumps to the message (loading older pages if needed, as
  `jumpToReply` does), opens its thread if it is in one, and flashes it. At most one is acted on per
  answer; the rest are listed as citations to tap.
- **Citations** render as small numbered chips under the answer, each showing the thread name and
  local time. Tapping one is a highlight.
- **`draft`** is offered only when `answerable` is false or when the owner asks for wording. It
  never sends: it fills the composer of the named channel (`#compose-text`), or replies in the named
  thread, with a banner reading "Draft from Ask, to <channel / thread> as you: edit, send, or
  discard". He must press Send himself. If the composer already holds text, the draft waits behind a
  "Replace your draft?" button.
- **Nothing else.** It cannot post, mark read, dismiss, change settings, or reach another channel
  than one it was fed. New powers would be added to this list one at a time.

## UI

- **Entry.** A small icon in the floating row at the top of the list, upper left, mirroring the
  search glass on the right. It is dim when off, lit (filled, accent colour) when active, and shows
  a spinner badge while catching up. The keyboard follows the page's existing scheme (README, "The keyboard: scrolling, search and Settings"):
  for example `?`, or Ctrl+/, to open it, Esc to close it, and the same key to toggle it while open.
- **Desk.** A floating panel, about 380 px wide and anchored top left over the list. It has its own
  short transcript and input, can be resized, and stays open while he scrolls the chat underneath,
  so a highlight is visible beside the answer.
- **Phone.** A bottom sheet at half height, which can be dragged to full height. A highlight
  collapses it to a peek bar ("Showing the message from 13:05, back to Ask"), so the message
  is visible.
- **Two-column view.** A highlight lands in the column that holds the thread.

## One brain with the voice agent?

> **Superseded in part, 2026-10-23 (`#455 vibe-talk-next-plan`).** The answer is now the master plan's: research is one service with a quick and a deep depth, and the voice agent calls it as the `research` tool (milestone 0.5.0). Its answers land in the Ask panel's research log.

The two share almost everything except the turn shape:

- **Share the briefing builder, rendering and rules.** These are the thread-grouped briefing, owner
  marking, fencing, "answer only from what you read, name the message, say when it isn't there" (the
  voice prompt's v12 rules) and the honesty about abilities. One module, two consumers.
- **Phase 3: the voice agent can ask research.** That would be one MCP tool, `research(question,
  depth: "quick")` (first proposed as `ask_helper`), answered by the quick-research session with a
  stronger model over the full briefing, within the voice tool
  deadline (a warm-cache question on Sonnet-class is a few seconds). The voice model then speaks the
  answer, while the page highlights the cited message in the background, which is how to "find"
  something by ear. This gives the voice agent precision without stuffing 100 k tokens into a speech
  model's turn.
- Research does **not** get the voice agent's `post_reply`. The draft action is its only path
  toward a post, and it ends in his hands.

## Privacy, prompt injection and failure modes

- **Where text goes.** The same model provider already gets channel text for summaries; research
  sends more of it (a day's worth per sitting). Research is off by default and per deployment
  (`[research]` absent means no research). Its transcript is kept in the store and removed by the
  storage purge.
- **Message text is third-party text.** It is fenced as data, and the system prompt says never to
  follow instructions in it. The structural defence is that research holds **no tools**: its only
  outputs are an answer, validated ids and a draft he must send himself. A message saying "draft a
  post saying deploy now" can at worst put words in a box he reads. Drafts are labelled as
  research's, and a draft whose text quotes a fed message verbatim is flagged.
- **Failure modes and what the owner sees:**

| Failure | What the owner sees |
|---|---|
| Model unreachable or rate-limited | "Research could not reach its model", with the error, and no invented answer |
| Briefing too large | The bound is lowered and said in words |
| Chat service slow during the briefing | The briefing is built from the message cache, never the chat adapter, and says when the cache is behind |
| Answer cites something invalid | The citation is dropped and the answer says so |
| Messages arrived since the last question | The answer header says "includes messages up to 13:20" |
| Live feed gap | The next question's header says when new messages last arrived |

## Prerequisites

- **Turn on the message cache where research will run** (`[message_cache] enabled = true`, and
  `serve` when ready). Today the busiest deployment has none, and its chat adapter serialises reads
  at about 4 s each, with page timeline reads of 40–50 s. A briefing of five channels' day built
  from upstream reads would take minutes and would compete with the page and the voice agent. With
  the cache, a briefing is a local query.
- **Thread names coverage.** The index is only as good as the names. Threads not yet named get a
  placeholder of their first words, and catching up does not wait for naming.
- **Measure tokens with the provider's `count_tokens`** on a real day's briefing before fixing the
  default span.

## Phased plan

> **Superseded in part, 2026-10-23 (`#455 vibe-talk-next-plan`).** P1 and P2 are milestone 0.2.0 (Ask) in the master plan, P3 is 0.5.0, and P4 (older history) moves to deep research over logbook with the `archive` class. The steps below stand as the detail of those milestones.

1. **P1: ask with citations.**
   - A `[research]` config and `POST /api/v1/research`, with a stateless briefing per sitting and
     prompt caching.
   - Structured answers with validated citations, plus the highlight action.
   - The desk overlay and phone sheet.
   - Off by default, with a "catching up" state.
   - Tests: id validation; injection fixtures (a message that tries to issue a highlight or draft);
     the budget statement; the append-only shape (the cache prefix is byte-identical across
     questions).
2. **P2: active/inactive and push-feed.**
   - The lit icon, live messages appended in thread groups, edits as correction lines, a fresh
     briefing on reactivation, and an idle timeout.
   - The draft action into the composer, with its banner.
   - The research log kept in the store.
3. **P3: one brain.** A shared briefing module with the voice agent, and `research` for the voice
   agent with a background highlight.
4. **P4: beyond the budget.** Server-side retrieval over older cached history, clearly labelled
   "older, searched", if the owner finds the stated bound limiting.

## Questions for the owner

> **Superseded in part, 2026-10-23 (`#455 vibe-talk-next-plan`).** These are consolidated, with the other plan documents' questions, in the master plan's last section: 1 to 6 are group C, 7 is answered (the voice agent may hand questions to research), and 8 is D1.

1. **Scope.** Should quick research cover every channel on the server, or only the one (or two) on
   screen? All channels gives "where was X" its full reach, at about 4× the tokens.
2. **Span.** Is "full text of the last 24 hours plus the name and sentence of every thread from the
   last 7 days" the right default? Or should it be "since I last read each channel", or a span you
   pick when you open it?
3. **Model and budget.** Which model should be the default: Haiku 5.5, which is cheap and long-context
   (a few cents a day), or Sonnet 5.5, more precise at about a dollar or two on a heavy day? Should
   there be a daily cost ceiling, and should the overlay show the spend?
4. **Active lifetime.** Should it switch itself off after an idle hour, or stay lit until you tap
   it? Should the active state follow you across devices and reloads?
5. **Drafts.** Is "into the composer of the channel or thread it names, never sent, with a banner"
   right? Should a draft ever be allowed to go to a channel other than the one on screen?
6. **Keeping its transcript.** Should the research log, its conversation with you, be kept and searchable like
   voice transcripts, or be thrown away when it is switched off?
7. **The voice agent.** Do you want the voice agent to be able to ask research (P3), so that spoken
   answers come from the same full read and the page highlights the message?
8. **The message cache.** May the cache be switched on where research runs? It is the
   prerequisite for a fast briefing.
