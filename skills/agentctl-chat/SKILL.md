---
name: agentctl-chat
description: Answer chat requests that the agentctl chat bridge types into your pane, and check what reached the chat. Use when a prompt opens with a header like "[from NAME via slack #channel · sent: ... · reply: <CHAT_REPLY_007>]" and holds a <chat_message> block (older bridges said "The user's request arrived through the configured chat bridge"), when you need to reply to a chat user, or when you need to confirm that a reply was sent.
---

# agentctl chat bridge: answering requests

The bridge (`agentctl chat run`) types each chat message from an allowed sender
into the coordinator's pane as a request prompt, and sends your replies back to
the chat thread. The original `allowed_senders` are permanent owners. Other
senders need an unexpired grant created by `agentctl chat grant --bridge-state
DIR SENDER --for DURATION`; `--all-members` grants the channel's explicitly
enumerated current members, and never future joiners. Every grant needs a
lifetime such as `30m`, `12h` or `7d`, within the configurable maximum (30 days
by default). `chat grants` lists active and expired grants and `chat revoke`
removes a temporary grant. A running bridge applies these changes without a
restart; permanent owners remain allowed.

This skill describes the compiled (Rust) bridge. Its request prompt is one
header line, the message between `<chat_message>` and `</chat_message>`, and a
short footer:

```text
[from Ada Lovelace via slack #team-chat · sent: 2026.10.30:10:51 EDT · reply: <CHAT_REPLY_007> or agentctl chat reply 007]

<chat_message>
the message
</chat_message>

Reply between a line <CHAT_REPLY_007> and a line </CHAT_REPLY_007>, ...
Earlier messages in this thread: agentctl chat thread 007
If you have trouble, run: agentctl chat help 007
```

`agentctl chat help 007` prints this request's rules and exact commands. The
short commands (`chat reply 007 --file FILE`, `chat sent 007`, `chat thread
007`) find the bridge that serves you on their own; when a prompt prints an
explicit command instead (with `--bridge-state` and `--request`, from an older
bridge or one that could not register), run it as printed. The Python edition
(`py/bin/agentctl chat`) words its prompts differently, has no `sent`, `thread`
or `help` command (it has `chat context --state`) and adds no send-time field
or delivery ✅; follow its own `--help` and the instructions in its prompts.

For the compiled bridge, use the installed command as the authority:

- `agentctl chat help ID`
- `agentctl chat sent ID`
- `agentctl chat thread ID --last 20`
- `agentctl chat userguide`

## Reply with the two marker lines

The prompt gives a reply ID and two marker lines:

```text
<CHAT_REPLY_007>
your answer, as many lines as you need
</CHAT_REPLY_007>
```

**This is the normal way to reply.** Put the opening line at the start of your
answer and the closing line after it, in your ordinary turn output. The text
between them reaches the user twice: in this terminal, where the owner may be
reading, and in the chat thread. Use the same two lines for every reply to that
request, including short progress updates during a long task. Each block with
new text is sent as one chat message; a block whose text the request has
already sent is not sent again, so give each progress update new wording. Make no tool call between the two lines. If the prompt
gave a numbered ID instead (`<CHAT_REPLY_<nonce>_1>`), add one to the number for
each later reply, as the prompt says.

## One request, several replies

A request takes any number of replies. Use several whenever an answer divides
by time or by topic:

- **By time.** When the work will take a while, acknowledge the request at
  once, send an update at each real step, and send the result as a reply of its
  own.
- **By topic.** When a message asks about three things, answer in three
  medium-sized replies, one per topic, not one combined wall of text.

Make each topic reply readable on its own, and open it by naming its topic.
Every reply uses that request's marker lines (numbered as the prompt says) or
its reply command, with new wording each time and one way of sending it.

Send each topic's reply as soon as it is ready rather than all at the end.
Several blocks in one output are posted only while the screen can show that
output from its first row through each block's closing line; when they will not
fit, send the later ones with the reply command. Keep it to a handful at a time:
after 8 replies to one thread within a minute, the bridge holds that thread's
next replies for at least 5 minutes.

## The reply command is the exception

When the bridge runs with `--offer-reply-command`, the header names `agentctl
chat reply ID` (write the reply to a file and add `--file FILE`), and `agentctl
chat help ID` gives the exact command. Use it only when the marker lines cannot
do the job:

- you must send something **before your turn ends**, and the text would not
  otherwise appear until then; or
- a block you wrote between the two lines did not reach the chat: the bridge
  reported it as not sent (a short notice in your pane, beginning `Chat reply
  not sent`), or `agentctl chat sent` does not list it, as when the screen was
  redrawn before the bridge read it.

Send each reply one way only. A reply sent by the command does not appear in
this terminal by itself; printing a copy without the marker lines sends nothing,
so it is a safe way to show the owner what you sent.

## Check what reached the chat

`agentctl chat sent ID` lists the replies the bridge holds for that request,
newest first (without an ID, every retained request's): reply number, phase,
the provider's message ID once it is sent, and when it was captured and sent.
Phase `sent` with a provider message ID means the chat provider accepted the
message. `pending` means captured and not yet sent; `sending` means a send is
in progress or its outcome is unknown, and the bridge retries it with the same
operation ID, so it is never posted twice. `agentctl chat thread ID` shows the
request's thread: its retained requests and replies in order. A retired
request's replies are no longer held, so the chat thread itself remains the
complete record. If a command cannot find the bridge, add `--bridge-state DIR`:
the state directory `agentctl chat help ID` names, or the `--bridge-state` of
the service's `agentctl chat run` command.

## What the reactions and the header mean

- The acknowledgement reaction (configured per deployment, for example 👀) is
  added when the bridge admits the message.
- ✅ is added, when the deployment has an acknowledgement reaction other than
  ✅, once the bridge's queue has typed the prompt into your pane and verified
  it against your composer, printed or queued behind another prompt. Adding it
  can fail or lag like any provider call, so a missing ✅ does not prove the
  prompt was not delivered. An older deployment whose acknowledgement reaction
  is itself ✅ uses it to acknowledge admission instead; it can appear before
  or after the prompt reaches you, and no separate delivery receipt is added.
- The header's `sent:` field gives the message's own send time, as
  `sent: 2026.10.07:08:45 EDT`, and adds `delivered 1 h 12 min later` when it
  reached you two minutes or more after that (older bridges opened the prompt
  with `Sent 2026.10.07:08:45 EDT.`). Several held messages can arrive
  together after an outage or a long turn; answer them knowing how old they
  are.

## Messages with files, and messages that come late

The Slack event stream does not carry a message with an attached file, such as
a screenshot. The Slack provider's periodic rescan delivers it instead, within
one rescan interval (90 seconds by default, `rescan_interval_seconds`) and
after messages the stream carried meanwhile, so check its send time. Such a
request's text ends with a line saying it came through the rescan. When a
message has files, a further note names each file ID with the exact command to
download it, for example:

```text
[agentctl] This message has 1 attached file: F0123456789. To view it, download to a temporary path with:
/path/to/slack-client slack.file download --file-id=F0123456789 --channel-id=C0123456789 --output-path=/tmp/agentctl-slack-files/
```

Run the command as printed and look at the file it reports saving, typically
an image you can read, before you answer. If the download fails, say so in your
reply rather than guessing what the file showed.

## Do not

- Do not use another chat tool or the provider's API to answer a bridged
  request; the bridge tracks replies per request.
- Do not write a request's marker lines around text you do not mean to send,
  such as an example or a quotation: the bridge reads them from your visible
  output.
- Do not answer a bridged request only in the terminal: the chat user sees
  nothing that is not between the two lines or sent by the reply command.
