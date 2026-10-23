# vibe-talk next: terminology

2026-10-23, `#455 vibe-talk-next-plan`. A proposal for the owner, used throughout the master plan
([README.md](README.md)) and now in the earlier plan documents. One page. Nothing here is a shipped
name except where it says "unchanged".

## The rule behind the names

Name each piece by **what it does with the data**, not by where it appears on screen. A UI surface
gets its own short label, and that label is never used for the machinery behind it. The test for
every term: could the voice agent, the page and a batch job all use the thing, and would the name
still be right?

## The recommended set

| Term | What it is | What it is not |
|---|---|---|
| **logbook** | The storage layer: one Rust crate holding what was said and done. A **chat store** (`Store<ChatMeta>`, provider messages, tombstones, coverage, watermarks) and a **transcript store** (`Store<TranscriptMeta>`, agent events), plus summaries, topics and search over both. The chat store is the server's one durable copy of provider messages: it is *the* cache. (Short-lived memories, such as the held recent windows, sit in front of it and report their age.) | Not the owner's own state (read marks, Done, bookmarks, hues, settings, the send log), which stays in vibe-talk's **owner store** (today's `StateStore`, unchanged). |
| **chat store** | logbook's half for chat. Replaces "message cache" once today's `src/cache/` moves into logbook (cutover C3). Until then "message cache" still names the code that exists. Its modes are **off**, **shadow** (fills and measures, and answers no page read; history consumers such as briefings and summaries still read it) and **serve** (also answers page reads it can prove current). | Not a second durable copy beside the cache. There is one. |
| **summary**, made by **the summarizer** | A model-written artifact about a **fixed unit**: one long message, a thread's name and sentence, an hour, a day, an agent phase, an agent's lifetime, a week's report. Its input is staged deterministically from logbook, it is content-addressed and cached, and it is rebuilt only when its input changes. It spends tokens, on a schedule (automatic, on demand, or batch) and within a budget. The **summary ladder** is the set of units, message to report. | Never answers a question. A summary of the runner-wedged thread is the same text whoever asks. |
| **briefing** | A deterministic, fenced rendering of what logbook holds for a scope and a span: topic list, thread index with names and sentences, availability line, newest messages, and (when the budget allows) full text grouped by thread. Building one spends no tokens. | Not a summary (no model writes it) and not a prompt by itself: it is the context a model is handed. |
| **research** | The open-ended question-answering service on the server ("where was X decided?", "what was decided for X?", "did X land?"). Messages API, no write tools, answers with validated citations and a statement of what it read. Two depths: **quick** (one model call over a briefing, warm prompt cache, seconds) and **deep** (a read-only tool loop over logbook, seeded with a briefing, tens of seconds to minutes, run as a **research job**). "Research agent" is acceptable shorthand for the service as a whole. | Not a summarizer (it takes arbitrary questions) and not a UI. Never posts. |
| **Ask panel** | The channel view's surface for research: the icon at the upper left, mirroring the search glass, opening a panel on a desk and a sheet on a phone. It holds the owner's **research log** (questions and answers), and it is the only place research's two UI actions, **highlight** and **draft**, act. Its label is "Ask". | Not the service. Research can be asked from the voice agent, with no panel open. |
| **voice agent** (unchanged) | The spoken front end: the voice model behind either voice backend, with its system prompt and its MCP tools. | Not a research engine. It talks, orients and delegates. |
| **Work tab** | The third view, beside Voice and Channel: the timeline of a channel's chat and of the agents behind it, drawn by the wrkviz timeline app from a logbook export. **wrkviz** (unchanged) names the tool, its archive format and its timeline app, Python today and Rust after the port. | Not wrkviz itself: the tab is one consumer of the export. |
| **availability bar** (unchanged) | The floating strip saying how much of the chosen window is summarized and caught up. | — |

Three words keep their existing narrow meanings: **catch-up** is the voice agent's
`recent_activity` read of recent full text (the Ask panel's "Catching up" is only a UI label for
building and warming its briefing); **digest** is the deterministic 160-character line format
(`summary.rs`), not a summary; **search** is the deterministic full-text index, which research uses
as one tool.

## How the voice agent relates to each

- **Consumes summaries.** A briefing at call start carries the topic list, the availability line and
  any caught-up hour summaries. At "fully ready" it gets a short day narrative. A `summaries` tool
  pulls any rung of the ladder. "What happened today?" is a summary question: it is answered from
  the ladder, cheaply, and never by research.
- **Calls research as a tool.** `research(question, channel?, depth?)` starts a job and returns at
  once. The answer comes back as a short spoken part, while the written part with citations lands in
  the Ask panel's research log. The voice agent never claims to have researched anything itself.
- **Reads chat itself only for the recent window**, through its chat tools (`recent_activity`,
  `read_page`, `find_message`, `read_message`), and posts only through `post_reply`.
- **Never touches the Work tab or transcripts directly.** Transcript content reaches a call only as
  research's short spoken answer, within the capability the owner set.

The dividing line for any question, by voice or in the panel: **a fixed question about a unit
("what happened today?", "what is this thread about?") is a summary; any other question is research,
even when it is about one thread ("what was decided in this thread?").**

## Rejected alternatives

| Name | For | Rejected because |
|---|---|---|
| **side agent** | The panel sits beside the chat | It names the screen position, not the job. The voice agent uses the same service with no "side" anywhere. Kept only as history in the file name `2026-10-22-side-agent-helper.md`. |
| **helper** | Short, friendly | Everything is a helper. It hides the one distinction that matters, a fixed unit versus an open question, and it reads as a second agent beside research when it is research's quick depth. |
| **assistant**, **assistant core** | Conventional | "Assistant" is already the voice agent's role to the owner and the Messages API's role name for model turns. Three meanings in one prompt. |
| **research agent** for the deep depth only, **helper** for the quick one | Matches the two earlier documents | Two names for one service with one prompt, one citation validator and one log. Depth is a parameter. |
| **librarian**, **oracle**, **analyst** | Memorable | A reader has to be told what each means. "Oracle" also promises certainty that a cited answer with a coverage statement deliberately does not. |
| **digest** or **recap** for summaries | Short | `digest` already names a deterministic line format and a REST route. Reusing it is how the voice agent's old tool confusion started. |
| **cache** for all of logbook | The owner's word for the store | Fits the chat store only. Transcripts are not a cache of anything, and an `archive` retention class is deliberately not disposable. "The chat store is the cache" is the precise sentence. |
| **memory** | Common in agent tooling | Suggests something a model writes for itself. logbook holds only what providers and transcripts say, plus derived summaries. |
| **Timeline tab**, **Agents tab** | Descriptive | "Timeline" is also the Channel view's word for Main and the wrkviz schema file, and "Agents" leaves out the chat lanes. "Work" says what it shows: the work behind the channel. |
