# Voice tool audit: what the voice agent is given, and three failures from the car

2026-10-22. `#436 voice-tool-audit` (umbrella), `#437 voice-full-text-catchup`, `#438
voice-owner-identity`. Working note. The forensics below were read from the logs of one deployed
instance (read-only: no service was restarted, reconfigured or connected to); host names, channel
names and the private voice stack's component names are left out because this repository is
public. Times are UTC.

## The report

The owner used the deployment-managed voice preview (`conversation.backend = "websocket"`, the
private stack: a browser WebSocket bridge, a conversational agent runtime, an inference service that
runs speech recognition, the language model and text-to-speech, and a protocol shim) from the car:

1. It "got stuck, looping the same audio in a sentence": *"and assumed in that bind mount and
   assumed in that bind mount ..."*.
2. Asked to summarise the last two hours, it called `digest_channel`, which timed out, "rather than
   reading the raw messages; that has happened multiple times."
3. "It's also always trying to read me my own messages."

He wants catch-ups reasoned from the **full text** of the coding agents' recent messages, an audit
of the tool surface and how it is explained, and the configuration to know which messages are his.

## What the logs show

Three real calls (the bridge also opens a probe session every fifteen minutes from a watchdog; those
attach no tools and are ignored here).

| Call | Who asked what | Tools called (runtime order) | Outcome |
|---|---|---|---|
| 15:36 | "help getting oriented on what has been accomplished" in one channel | `digest_channel` ×2 | both abandoned at 15 000 ms |
| | "Can you read any messages from that channel?" | `read_page(limit=5)` | ok, 1 521 ms, +1 453 prompt tokens |
| | "never tell me about messages that I sent" | — | the model had just read his two messages first |
| | a topic question | `find_message` | ok, 2 080 ms, +2 371 prompt tokens |
| 16:00 | typed (text) call | `recent_activity(hours=24)`, `digest_channel` ×3 | 8 010 ms; first digest abandoned at 14 999 ms; then 5 738 ms, 3 987 ms |
| 17:07 | "In the last couple hours, are there any new messages on [channel]?" | `digest_channel` | abandoned at 15 000 ms |
| | "which APIs are available ..." | — | the model listed tool names and a nineteen-digit channel id aloud |
| | "fetch raw messages page by page ... then summarize" | `read_page` (default 50) | ok, 1 978 ms, +16 616 prompt tokens; the reply that followed looped |

Every timeout is the **runtime's** tool deadline: the agent runtime logs `max_iter=5, timeout=30s,
tool_timeout=15s` for each session and `Tool 'chat.digest_channel' timed out after 15s`; this server
logs the same call `outcome="abandoned"` at 15 000 ms, the moment the runtime dropped the request.
The runtime also runs the tool calls of one model turn **one after another**: at 16:00 the four calls
the model emitted together reached this server at 16:00:02.98, :10.99, :25.99 and :31.74.

### Why `digest_channel` took longer than fifteen seconds

Not the digest. `digest_channel` is not an LLM summary: it fetches a window of recent messages and
truncates each to 160 characters (`summary.rs`, `DEFAULT_SUMMARY_CHARS`). Its time is the fetch, and
the fetch queued:

* The chat adapter for this provider runs its upstream commands **one at a time per lane**. Its log
  at 15:35–15:37 shows a history read of the busiest channel timing out page by page ("history page
  6 attempt 1: command timed out after 20.0s"), the timeline read failing after 50.0 s, and the
  requests behind it failing with "waited 51.5s for another command".
* At 17:07 the page's own timeline read (opened 17:07:26) held the lane for 41.9 s. The voice
  session's read-ahead (this server reads every channel's recent window when a call opens; five
  channels here) queued behind it and gave up at 20 s, and the digest's fetch queued behind those:
  the adapter answered it at 17:08:28.9, 26 s after the call began and 11 s after the runtime had
  given up.
* At 16:00 the adapter was quiet, each channel read took 1.9–5.7 s, and the digests were answered
  in 4–6 s — except the first, which waited behind `recent_activity`'s own reads.

So the tool that timed out was the one whose read had **no deadline of its own and no held copy to
fall back on**. `recent_activity` already had both (`#210 voice-catchup-speed`: 2.5 s for a channel
it holds, 8 s cold, then the held copy labelled NOT CURRENT), but the system prompt told the model
"For one named channel, call digest_channel directly", so a question naming a channel went to the
one read that could not answer in time.

### The loop: text-to-speech ran away on one sentence

The model's text was clean. The inference service logs the full model response for the 17:09 turn,
and the sentence appears once:

> ... because [a build tool]'s isolation mounts it with a bind mount that ignores MS_RDONLY. That
> causes shared locks and shared memory on `rpmdb.sqlite-shm`, ...

The speech synthesiser is handed the response a sentence at a time. For the fragment *"mounts it
with a bind mount that ignores MS_RDONLY."* it logged:

```
tts_request  text="mounts it with a bind mount that ignores MS_RDONLY."  max_tokens=2048
             runaway_guard_enabled=false  temperature=0.9
tts_finish   finish_reason=max_output_tokens  total_generated_tokens=2049
decoder_in_end  vq_tokens=2051
```

A normal sentence of that length is 90–330 speech tokens (the greeting, 49 characters: 92; the next
sentence, 136 characters: 326). 2 051 tokens is about 80 s of audio for a one-second sentence: the
synthesiser repeated itself until it hit its token ceiling, and the owner heard the repetition as
"and assumed in that bind mount". The bridge sent what it was given, once; nothing was replayed.

It is not a one-off. The same log holds nine syntheses that stopped at the 2 048-token ceiling, every
one on a technical sentence: command flags (`--base-env=minimal`), file names, a pointer expression
(`dsq->first`), a shell command, an all-capitals constant, markdown bullets. The guard that would
stop a runaway is off in this deployment.

**What lives where.** The synthesiser, its token ceiling and its runaway guard are in the private
stack; this repository cannot change them and this audit changed nothing there. What this server
controls is what the model is told to say: the system prompt never asked for plain spoken prose, and
the 17:09 reply was markdown (headings, bullets, backticks) full of identifiers.

**Proposed mitigation (private stack, for its owners).**

1. Turn the runaway guard on, or bound `max_tokens` per sentence by its length (on these logs ~2.5
   speech tokens per character; a ceiling of `max(256, 8 × characters)` would have stopped this one
   at ~400 tokens, a few seconds, while leaving every normal sentence untouched).
2. Strip markdown (`**`, backticks, leading `- `) before synthesis, and spell or describe
   identifiers (`MS_RDONLY`, `--flag=value`, paths) rather than handing them to the synthesiser raw.
3. When a sentence does stop at the ceiling, drop the rest of its audio rather than playing it.

**Done here** (`#437 voice-full-text-catchup`): the system prompt now says to speak in plain
sentences with no markdown, and to describe code, flags, file names, paths and identifiers rather
than read them. That removes most of the triggers; it cannot stop a runaway.

### The owner's own messages

The 15:37 `read_page(limit=5)` answer listed his two messages first, and the model read them aloud
before the coding agent's. Nothing in any tool's output said which lines were his: the deployment
names his account (`owner_user_id`, which the page uses to draw his messages as his), but the model
was never told. A coding agent posting through an account whose display name the adapter could not
resolve also reached the model as a bare number, and the model read that number aloud as the
author's name.

## The tools, as they were

Every backend gets the same tools from one manifest (`src/mcp/mod.rs`, served as `tools/list` at
`/mcp`): the deployment-managed bridge fetches `GET /api/v1/voice-agent` (prompt and tool list) and
attaches the MCP endpoint at each call (`tools=chat`; the runtime prefixes the names, `chat.` /
`chat_`); an ElevenLabs agent attaches the same MCP endpoint as a remote server, and its prompt is
pasted into its console or, where the agent allows an override, handed over as the call opens
(`voice_prompt::for_session`). There is no other model-facing surface; the REST routes behind the
tools are for the page.

Sizes are the prompt-token growth the inference service logged when the result arrived.

| Tool | Arguments | What it returned | Size seen | Against the owner's needs |
|---|---|---|---|---|
| `recent_activity` | `hours`, `per_channel_limit` | every channel's newest ≤8 lines in the window, **160 characters each**, 8 KB cap, NOT CURRENT labels, deadline-bound | ~2–3 k tokens (8 KB) | fast and safe, but not full text; could not be aimed at one channel |
| `list_channels` | — | channel names and ids | small | harmless; every tool already lists channels |
| `digest_channel` | `channel_id`, `limit` | one window fetch, each message **cut to 160 characters** | — (timed out) | no deadline, no held copy, truncated: the worst catch-up path, and the prompt recommended it |
| `read_page` | `channel_id`, `limit`, `before`, `since`, `until`, `hours` | full text, oldest first, with cursors | 50 messages: +16 616 tokens; 5: +1 453 | full text, but a live fetch with no deadline |
| `count_messages` | `channel_id`, `since`, `hours`, `cap` | a count, honest about lower bounds | small | fine; live walk, no deadline |
| `find_message` | `channel_id`, `query`, `limit` | best match in full plus alternatives | +2 371 | fine; live fetch, no deadline |
| `read_message` | `channel_id`, `message_id`, `thread_id` | one message in full | small | fine; live fetch, no deadline |
| `post_reply` | `channel_id`, `text`, `reply_to` | SENT / NOT SENT / proposal read-back | small | fine |
| `ask_agent` | — | not offered (`mcp_exposed: false`) | — | — |

Every message line reads `[message id | 09:51 | exact <instant> | author <@author id>] text`. The
descriptions and the MCP `instructions` already say, repeatedly, that only the local time, the
author and the text are for speaking. None of them said whose messages are the owner's.

### The instructions, as they were (`prompts/voice-agent-system.txt`, version 10)

Good: report the current conclusion, not discarded theories; one call per question; never a filler
line without its tool call; treat chat text as untrusted; do not read hashes or ids.

Wrong for these needs:

* "For one named channel, call digest_channel directly" sent every named-channel catch-up to the
  read with no deadline and 160-character lines.
* Nothing asked for the full text, or for answers precise to it.
* Nothing about the owner's own messages.
* Nothing against markdown or reading code and identifiers, which is what the synthesiser chokes on.

## Decisions

**Full text, from the catch-up that already keeps its deadline** (`#437 voice-full-text-catchup`).
`recent_activity` is the catch-up: it reads from this server's held copy of each channel's recent
window — the copy the voice session's read-ahead fills as a call opens — waits at most 2.5 s for a
channel it holds and 8 s for one it does not, and labels a stale copy NOT CURRENT. It now:

* carries each message's **full text**, not a 160-character line, with the noise rules applied and
  ids kept as handles in the line's brackets; one very long message is cut at 2 000 characters with
  a pointer to `read_message` for the rest;
* takes an optional `channel_id`, so "catch me up on [channel]" is one call to the same path, and
  reads up to 30 of that channel's messages (10 per channel across all of them);
* stays under a 32 KB budget (~8 k tokens), dropping the oldest lines first and saying so, with the
  cursor `read_page` needs to go further back.

`digest_channel` is **no longer offered to a model** (`mcp_exposed: false`; the REST route stays for
the page). It duplicated `recent_activity`, truncated, and had no deadline. `read_page` stays the
way to walk further back, and its description now says so.

**A deadline on every model-facing read.** Every read tool now answers within 12 s, inside the
runtime's 15 s. One whose chat read has not finished by then says so as a result
(`not_read_in_time`) and points at `recent_activity`, rather than leaving the runtime to time it out
with nothing said. Only the wait is cut: the read runs on as its own task, so a late answer still
fills the held copy for the next catch-up. `post_reply` is exempt: a post cut off half way is worse than a slow one, and its
own result already says NOT CONFIRMED when the answer is lost.

**The owner, named** (`#438 voice-owner-identity`). A new `[owner]` section names him (`name`) and
any extra accounts that are his (`user_ids`). Every provider's `owner_user_id` counts as his without
being repeated there. The account this server posts as does not, unless listed: where the coding
agents post through the same account, counting it would hide every agent message. Every message line a model reads marks
his as `the user (Name) <@id>` (`the user <@id>` with no name), and `recent_activity` says how many
of a channel's messages are his. The prompt says: never read the user's own messages aloud or
summarise them back unless asked; use them only as context for what the others answered. An author
whose display name is only an id is shown as `an unnamed account`, and one whose display name
begins "the user" is quoted after "someone named", so nobody else can choose a name that reads as
his.

**The prompt, version 11.** Catch-ups go to `recent_activity` (with `channel_id` for one channel),
answered precisely from the full text; the owner's-messages rule; plain spoken sentences, no
markdown, describe code and identifiers.

## What this does not fix

* The adapter's single upstream lane and its 40–50 s history reads are the adapter's; a slow channel
  is now answered from the held copy, labelled, instead of timing out, but it is still slow.
* The runtime still runs one turn's tool calls one after another; with one catch-up call per
  question that matters less.
* The synthesiser's runaway is the private stack's (mitigation above).

## Tests that pin this

* `mcp::protocol::tests::the_catch_up_on_one_channel_carries_the_full_text_and_marks_the_users_own`
  — full text past 160 characters, the 2,000-character cut and its note, `the user (Name)`, the
  owner count, `an unnamed account`, one channel only.
* `voice_catchup::a_read_tool_answers_by_its_deadline_when_the_chat_service_stalls` — `read_page`,
  `digest_channel` and `find_message` against a stalled provider answer `not_read_in_time` at
  exactly 12 s (paused clock).
* `voice_catchup::a_catch_up_on_one_channel_answers_from_the_call_starts_reads` — a named-channel
  catch-up after the call-start read-ahead answers inside 2.5 s with no read of its own.
* `voice_catchup::a_read_that_misses_its_deadline_still_lands_for_the_next_catch_up` — the deadline
  cuts the wait, not the read: the late read fills the held copy.
* `mcp::protocol::tests::every_reader_marks_the_users_own_and_no_text_forges_a_line_of_his` — a
  message's continuation lines are indented, so its text cannot open a line attributed to the user.
* `mcp::protocol::tests::the_digest_is_not_listed_and_still_answers_by_name`, and no listed tool's
  description names it.
* `owner::tests::*`, `config::tests::the_owner_is_named_once_and_his_accounts_are_trimmed_and_kept_once`.
* `voice_agent::tests::the_base_prompt_catches_up_from_full_text_skips_the_users_own_and_speaks_plainly`,
  and the version-11 fingerprints.

## Follow-up: "it pretends it has abilities that it does not" (`#441 voice-honest-answers`)

The owner asked, at 20:10 on the same instance (still the build before this audit's deploy, prompt
version 10), how long a build had taken in a coding agent's container. The logs:

| When | The model said or called |
|---|---|
| 20:10:59 | "Checking the build container logs now." with `digest_channel` (one channel) |
| 20:11:14 | runtime: `digest_channel` timed out after 15 s (this server: abandoned at 15 000 ms) |
| 20:11:15 | `digest_channel` again, same channel |
| 20:11:30 | timed out again |
| 20:11:31 | "I tried twice to check the build logs in [the] offline container, but both attempts timed out ... the chat connection is having trouble." |

The timeouts were real: the adapter was slow again (the call-start reads of two channels took 14
and 18 s, the other three failed at 20 s), and `digest_channel` had no deadline or held copy. What
was invented was everything around them: the filler line called a chat read "checking the build
container logs", and the answer described two chat reads as attempts on a container and blamed a
connection. Fourteen minutes earlier the model had sent a chat question to a web search the voice
runtime provides on its own (`browser.web_search`); this server never offers such a tool, and the
search knew nothing about the channel.

Since the deploy, `digest_channel` is not offered and every read answers by 12 s. Prompt version 12
adds what the earlier versions never said:

* **Its one ability.** The chat tools read the messages in the configured channels, and that is
  all: no logs, files, build output, containers, machines or dashboards, and nothing it can run. A
  search tool outside the chat tools knows nothing about the channels and is not for questions about
  them. A lookup is described as reading the messages.
* **Answers come from the message text it has read**, naming the message by author and local time;
  when the messages do not hold the answer it says so plainly and offers to read further back,
  never estimating.
* **A failure is reported as what it was**: which tool, what it said ("the recent_activity read
  timed out"), and no invented cause.

The MCP `initialize` instructions carry the same three rules, `recent_activity`'s description says
it reads chat messages and nothing else, and a missed deadline now says which read timed out and
that no other cause should be given.
