---
name: agentctl-chat
description: Answer chat requests that the agentctl chat bridge types into your pane, and check what reached the chat. Use when a prompt says "The user's request arrived through the configured chat bridge", when you need to reply to a chat user, or when you need to confirm that a reply was sent.
---

# agentctl chat bridge: answering requests

The bridge (`agentctl chat run`) types each chat message from an allowed sender
into the coordinator's pane as a request prompt, and sends your replies back to
the chat thread. Use the installed command as the authority:

- `agentctl chat quickstart`
- `agentctl chat userguide`
- `agentctl chat sent --bridge-state DIR`
- `agentctl chat thread --bridge-state DIR --thread THREAD --last 20`
- `agentctl chat reply --help`

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
request, including short progress updates during a long task; each block is
sent as one chat message. Make no tool call between the two lines. If the prompt
gave a numbered ID instead (`<CHAT_REPLY_<nonce>_1>`), add one to the number for
each later reply, as the prompt says.

## The reply command is the exception

When the bridge runs with `--offer-reply-command`, the prompt also prints an
exact `agentctl chat reply ... --file PATH_TO_YOUR_REPLY` command. Use it only
when the marker lines cannot do the job:

- you must send something **before your turn ends**, and the text would not
  otherwise appear until then; or
- a block you wrote between the two lines was reported as not sent (the bridge
  may type a short notice into your pane, beginning `Chat reply not sent`).

Send each reply one way only. A reply sent by the command is not shown in this
terminal unless you also print it, which would send it twice; say in your turn
output that you sent it, without the markers.

## Check what reached the chat

`agentctl chat sent --bridge-state DIR` lists the replies the bridge holds,
newest first: request, reply number, phase, the provider's message ID once it is
sent, and when it was captured and sent. Phase `sent` with a provider message ID
means the chat provider accepted the message. `pending` means captured and not
yet sent; `sending` means a send is in progress or its outcome is unknown, and
the bridge retries it with the same operation ID, so it is never posted twice.
`agentctl chat thread` shows one thread's retained requests and replies in
order. A retired request's replies are no longer held, so the chat thread
itself remains the complete record. The prompt gives the exact state directory
in its `chat thread` command.

## What the reactions and the first line mean

- The acknowledgement reaction (configured per deployment, for example 👀) is
  added when the bridge admits the message.
- ✅ is added once the bridge's queue has typed the prompt into your pane and
  verified it against your composer, printed or queued behind another prompt.
- Each prompt's first line opens with the message's own send time, as
  `Sent 2026.10.07:08:45 EDT.`, and says `delivered 1 h 12 min later` when it
  reached you two minutes or more after that. Several held messages can arrive
  together after an outage or a long turn; answer them knowing how old they
  are.

## Do not

- Do not use another chat tool or the provider's API to answer a bridged
  request; the bridge tracks replies per request.
- Do not write a request's marker lines around text you do not mean to send,
  such as an example or a quotation: the bridge reads them from your visible
  output.
- Do not answer a bridged request only in the terminal: the chat user sees
  nothing that is not between the two lines or sent by the reply command.
