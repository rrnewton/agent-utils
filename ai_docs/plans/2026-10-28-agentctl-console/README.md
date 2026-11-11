# agentctl console: the owner's input line for a headless agent

Design note for #670 agentctl-console, written for the owner's review before any code is
written. Date: 2026-10-28. Status: proposal.

## The request

The owner wants to type, or paste, straight into the terminal that follows a headless agent,
and have it arrive "the same as `agentctl send`", but read by the harness as a prompt from the
owner. Addenda from the same request: a flag on `tail` would also do; keep it a plain console
(no full-screen interface); Enter sends a line, and bracketed paste makes a multi-line paste one
message; console input is a plain user turn, with no agent or bridge framing; the console
checks that a person at a terminal is typing, so a lane cannot use it to pass off its own
instructions as the owner's; every console input is logged with where it came from; the console
answers #671 headless-approval-surface prompts (`approve aN` / `deny aN`).

## What exists today

- The headless wrapper hands every queued input to Claude Code unchanged, as one stream-json
  line (`headless/claude.rs`, `input_line`):

  ```json
  {"type":"user","message":{"role":"user","content":"TEXT"},"uuid":"…","session_id":"…","parent_tool_use_id":null}
  ```

  All framing therefore lives in TEXT, added by whoever queued it:

  | Route | TEXT the harness receives |
  |---|---|
  | `agentctl send` | `[from human@host via agentctl]` or `[from NAME via agentctl · reply: agentctl send NAME]`, a newline, then the message (#609 sender-header-mandatory; no option removes it) |
  | chat bridge | an `[at: …]` time opening, "The user's request arrived through the configured chat bridge.", the chat context, the message, then the reply instructions and reply command |
  | watch-agents | `[agentctl watch-agents] …` |

- The queue item's `from` field is taken from `AGENTCTL_AGENT`, i.e. on trust, and the queue
  directory can be written by any process of the same user.
- `agentctl approve|deny` (#671) already refuses unless standard input and output are
  terminals and `AGENTCTL_AGENT` is unset. It answers over the wrapper's control socket (mode
  0600), and the wrapper refuses any peer that descends from the harness process
  (`SO_PEERCRED`; `approval::descends_from`).
- `agentctl tail NAME --follow` is documented as read-only, lock-free and writing nothing, and it
  is used in pipes and scripts.

## Recommendation: a new `agentctl console NAME`; `tail` stays as it is

`console` = `tail --follow` plus an input line at the bottom. It takes tail's display options
(`--last`, `--types`, `--width`, `--plain`, `--tool-lines`) and the same renderer.

Why not `tail --input`: tail's contract (read-only, no lock, works in a pipe, any number of
copies) is what makes it safe to leave running anywhere, and an input flag would turn the one
command into two with different requirements (console refuses to run without a terminal). Why
not rename tail with an alias: scripts and the watcher's hints say `tail`, and a rename buys
nothing over a sibling command. The Herdr tab that runs `agentctl tail coordinator --follow`
switches to `agentctl console coordinator` when the owner wants it to.

Scope: local headless agents only. A pane agent already has a terminal (`agentctl attach`).

## Input semantics

The console puts the terminal in raw mode (termios through `libc`, already a dependency; no new
crate). It restores the terminal on exit, panic, SIGTERM and SIGHUP. It turns bracketed paste
on (`ESC[?2004h`) and off again on exit.

| Key | Effect |
|---|---|
| printable text | inserted at the cursor |
| Enter | sends the buffer as one message (trailing white space trimmed; an empty buffer sends nothing) |
| paste (`ESC[200~` … `ESC[201~`) | inserted into the buffer verbatim, newlines kept (CR becomes LF), **not** sent; shown as `[paste: 14 lines, 812 bytes]` so the line stays one row; Enter then sends everything as one message |
| Ctrl-J | inserts a newline by hand (for terminals without bracketed paste, where each pasted line would otherwise arrive as an Enter) |
| Backspace, Delete, Left, Right, Home/Ctrl-A, End/Ctrl-E, Ctrl-U, Ctrl-K, Ctrl-W | ordinary line editing |
| Ctrl-C | clears a non-empty buffer; on an empty buffer exits the console |
| Ctrl-D | deletes under the cursor; on an empty buffer exits the console |

Exiting the console never touches the agent. The console has no history, completion or
multi-row editor in the first version.

Events keep scrolling above the input line: before each event the console clears the input row
(`\r ESC[K`), prints the event, and redraws the prompt (`NAME › `) and the buffer. A sent message
prints a local acknowledgement (`queued q-…`). Its `input.queued` and `input.started` events
then appear in the stream like any other input.

Size: one message is at most 48 KiB of text. The control socket reads request lines of up to
64 KiB, and JSON escaping needs room. A larger buffer is refused locally and kept, so nothing is
lost.

A one-line harness slash command (`/compact`) is sent as written, as with `agentctl send`.

## Delivery path: the wrapper queues console input itself

The console does not write the queue directory. It sends one control request:

```json
{"op": "console.input", "text": "TEXT", "user": "alice", "tty": "/dev/pts/7"}
```

The wrapper:

1. checks the peer (`SO_PEERCRED`): the same user ID as the wrapper, and not the harness or any
   descendant of it (the same check #671 applies to approvals);
2. queues TEXT in the agent's own queue (inbox.v2 lane, `normal` priority, delivered at the next
   turn boundary, the same as a plain `agentctl send`), with `from: "console:USER"` and an `origin`
   object (`kind: console`, user, uid, tty, peer pid, session id);
3. logs `input.queued` with the same `from` and `origin`, and answers `{"ok": true, "queue_id": …}`;
4. delivers it like any queued input, so the harness receives exactly

   ```json
   {"type":"user","message":{"role":"user","content":"TEXT"},…}
   ```

   with TEXT byte for byte as typed: no sender header, no time opening, no bridge text. That is
   the same shape Claude Code produces for a prompt typed at its own terminal.

Queueing in the wrapper rather than the console matters because the wrapper can check who is
on the other end of the socket, and the queue's self-reported `from` proves nothing. When the
wrapper is not running, the console says so and keeps the buffer: it does not queue for later,
because nothing could check the input when it is finally delivered.

## Authentication: what the checks prove and what they do not

The console refuses to start unless all of these hold:

1. standard input and standard output are terminals (not a pipe or a file);
2. standard input is the process's controlling terminal (`tcgetsid(0) == getsid(0)`), and the
   console is in that terminal's foreground process group;
3. `AGENTCTL_AGENT` is unset or empty;
4. no ancestor process is a coding-agent harness (`claude`, `codex`, `muse`), checked with the
   ancestor scan `sender_header` already does.

The wrapper then repeats the peer check above for every input.

Proved: the input came from a process of the owner's user ID, on a real terminal it controls,
outside every agent's process tree, not identifying itself as an agent. That stops a lane from
running the console by accident, or because text it read told it to. It also gives every
console turn an origin in the log.

Not proved: that a person typed it. Any process of the same user can make a terminal and type
into it (a Herdr or tmux pane started outside the agent's tree, then `send-text`). It could also
skip the console and write the queue directory directly. Same-user code is not a boundary here,
the same position #671 `approval.rs` states. A "press y to confirm" key would not change that: the
same process can type the key. I recommend against it because it adds friction and proves
nothing.

## Logging

- Event log: every console input is an `input.queued` event with `from: console:USER` and the
  `origin` object. The full text stays in the queue item, which is kept under `processed/`; the
  event holds the usual 600-byte excerpt. `tail` and `console` render it as
  `from console:alice (pts/7)`.
- A request the wrapper refuses (peer inside the harness tree, wrong uid) is logged as a
  `console.refused` event: peer pid, reason, and a masked excerpt. That records a laundering
  attempt. A refusal on the console side (checks 1-4) prints to standard error only, because
  nothing reached the agent.
- Approval answers from the console are the existing `approval.decided` with
  `by: console:USER`.

## Approval answers

A line that `approval::parse_reply` reads as an answer that names an ID (`approve a7`,
`deny a7 too broad`) goes to the wrapper as the existing `approval` request with
`by: console:USER`, the path `agentctl approve|deny` takes. If a7 is not waiting, the console
prints that, and sends nothing. A bare `approve` with no ID gets a hint to name it (the same rule
the chat path follows: an answer must name the prompt the owner was shown). An answer-shaped
line is never delivered to the agent as a message. Ordinary text is unaffected, because
`parse_reply` matches only those exact short forms.

`approval.requested` events render in the console with a console hint ("type `approve a7` or
`deny a7 [reason]`"). The console can approve calls whose detail was cut or masked for chat,
since it shows the call in full; #671 already allows that for console answers.

## With a chat bridge attached

- Both feed one queue lane. The wrapper takes inputs oldest first, one per turn boundary. Console
  and chat inputs interleave in order of arrival, and neither overtakes the other. A console
  input queued ahead of a chat message delays it by one turn, as any queued input does.
- A console turn carries no reply instructions, and the bridge's reply reminders key on the chat
  requests it queued. The agent's answer to a console turn therefore shows in the console and is
  not posted to the channel unless the agent chooses to.
- Approvals: the first answer wins, from chat or console. The later one gets "not waiting" and
  changes nothing.

## Checking the guardrail behaviour (no security plugin is modified)

What the guardrail's local JSONL event log shows today (read-only scan of its last ~6 GB,
2026-10-28; field names as the plugin writes them):

- 7,333 `UserPromptSubmit` events; none sets or changes a session fact.
- 777 events set `ExternalContentIngested`, all `PostToolUse`: 716 Bash commands that fetch
  (curl, `git fetch`, herdr-run git), 61 web searches and knowledge loads.
- The fact then stays true for the session. In the headless coordinator session sampled, it was
  already true at the start of the window and was re-set by a `git fetch`.
- All 39 denials in the window had it true. Their reasons include
  `external_instruction_following` ("command embeds directive from ingested external content …
  unrelated to user's stated goal") and `unprompted_dangerous_action`.

So the console cannot clear a taint that a tool's output has already set. What it changes is how
the owner's instruction arrives. It is the user's turn word for word, so it becomes the "user's
stated goal" the guardrail checks actions against. A prompt framed as relayed chat or agent
content does not get that standing. Whether that changes decisions has to be measured, not
assumed:

1. **Bytes (automated, in the test suite):** with a fake harness, assert that a console input
   reaches the harness's standard input as exactly the stream-json line above with TEXT
   unchanged, while `agentctl send` of the same text carries the header.
2. **Live (with the owner's go-ahead, on a scratch headless agent, never the coordinator):**
   read the session ID from the agent's record. Select that session's events from the
   guardrail log with `tail -c … | grep -F SESSION_ID` (the log is ~17 GB, so never read it
   whole). Then:
   - console prompt "run `date`": the `UserPromptSubmit` `user_prompt` equals the typed text,
     and the next `PreToolUse` has `ExternalContentIngested: false` and a `UserIntent` that
     paraphrases the owner;
   - the same text through `agentctl send`, then through the bridge: compare `UserIntent` and
     the facts;
   - set the taint deliberately (one `git fetch` from GitHub, check that it latches true). Then
     ask for one harmless owner-directed action, first through the bridge and then through the
     console, and record `decision`, `matched_rule` and `reason` for each.

   The write-up records booleans, rule names and decisions only, never prompt text.

## Open questions for the owner

1. `agentctl console` as a separate command (recommended), or `tail --follow --input`?
2. Mid-turn delivery: console input waits for the turn boundary, like `agentctl send`. Do you
   want a key that delivers between tool calls (urgent), and Esc to interrupt the turn as Claude
   Code's own terminal does? Neither is in the first version as proposed.
3. When the wrapper is down, refuse and keep the buffer (recommended), or queue for later?
4. Separate issue, if wanted: should the wrapper add a header to queue items that carry none and
   did not come from the console? That would make a plain turn something only the console
   produces, and close the "write the queue directory directly" path for well-behaved tools.
